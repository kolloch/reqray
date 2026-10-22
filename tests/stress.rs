//! Randomized tests of the accounting, through the public API.
//!
//! `single_thread_matches_oracle`: random programs of span creation, entry
//! (also again), exit in any order and drops on one thread, with the mock
//! clock, compared with an oracle that spends every clock increment by the
//! definition of the columns: busy for every entered span, own for the
//! span entered last, alive for every span not yet closed.
//!
//! `threads_keep_the_invariants`: the same operations on four threads at
//! once, with spans shared across threads, a real clock and a `fmt` layer
//! that reads span extensions for every event: no panic, no deadlock,
//! every root reported once, own within busy, busy within alive on every
//! thread, depths consistent.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use quanta::Clock;
use reqray::{CallPathPool, CallPathTiming, CallTreeCollectorBuilder, FinishedCallTreeProcessor};
use tracing::{Dispatch, Span, info, info_span, span::EnteredSpan};
use tracing_subscriber::prelude::*;

const MAX_CALL_DEPTH: usize = 4;

/// A span's callsite, by name and line (two callsites may share a name).
type Callsite = (&'static str, u32);
/// A call path: the callsites from the root.
type Path = Vec<Callsite>;
/// (calls, alive, busy, own) in nanoseconds of every call path of a tree.
type Sums = BTreeMap<Path, (usize, u128, u128, u128)>;

/// xorshift64*, enough for choosing operations.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

#[derive(Clone, Default)]
struct Pools(Arc<Mutex<Vec<CallPathPool>>>);

impl FinishedCallTreeProcessor for Pools {
    fn process_finished_call(&self, pool: CallPathPool) {
        self.0.lock().unwrap().push(pool);
    }
}

/// How a new span gets its parent.
enum Parent<'a> {
    /// A root.
    None,
    /// The current span of the thread, if any.
    Current,
    Explicit(&'a Span),
}

const KINDS: usize = 4;

/// A span of one of the callsites (call paths are chains of callsites, so
/// the contextual ones have names of their own, and kind 3 is a second
/// callsite named like kind 0), and its callsite.
fn make_span(kind: usize, parent: Parent<'_>) -> (Span, Callsite) {
    let span = match (kind, parent) {
        (0, Parent::None) => info_span!(parent: None, "a"),
        (1, Parent::None) => info_span!(parent: None, "b"),
        (2, Parent::None) => info_span!(parent: None, "c"),
        (_, Parent::None) => info_span!(parent: None, "a"),
        (0, Parent::Current) => info_span!("ca"),
        (1, Parent::Current) => info_span!("cb"),
        (2, Parent::Current) => info_span!("cc"),
        (_, Parent::Current) => info_span!("ca"),
        (0, Parent::Explicit(parent)) => info_span!(parent: parent, "a"),
        (1, Parent::Explicit(parent)) => info_span!(parent: parent, "b"),
        (2, Parent::Explicit(parent)) => info_span!(parent: parent, "c"),
        (_, Parent::Explicit(parent)) => info_span!(parent: parent, "a"),
    };
    let meta = span.metadata().expect("the span is enabled");
    let callsite = (meta.name(), meta.line().unwrap());
    (span, callsite)
}

fn callsite(timing: &CallPathTiming) -> Callsite {
    let meta = timing.static_span_meta();
    (meta.name(), meta.line().unwrap())
}

/// The sums of a pool by call path, checking the structure on the way.
fn sums(pool: &CallPathPool) -> Sums {
    fn walk(pool: &CallPathPool, id: reqray::CallPathPoolId, path: &mut Path, out: &mut Sums) {
        let timing = &pool[id];
        path.push(callsite(timing));
        assert_eq!(timing.depth(), path.len() - 1, "depth of {path:?}");
        assert!(timing.depth() < MAX_CALL_DEPTH, "{path:?}");
        assert!(timing.call_count() >= 1, "{path:?}");
        assert!(
            timing.sum_without_children() <= timing.sum_with_children(),
            "own within busy at {path:?}: {timing:?}"
        );
        let previous = out.insert(
            path.clone(),
            (
                timing.call_count(),
                timing.span_alive().as_nanos(),
                timing.sum_with_children().as_nanos(),
                timing.sum_without_children().as_nanos(),
            ),
        );
        assert!(previous.is_none(), "call path {path:?} twice");
        for child in timing.children() {
            // Every node comes after its parent.
            assert!(*child > id, "{path:?}: child {child:?} before {id:?}");
            walk(pool, *child, path, out);
        }
        path.pop();
    }
    let mut out = BTreeMap::new();
    let mut path = Vec::new();
    walk(pool, pool.iter().next().unwrap().0, &mut path, &mut out);
    assert_eq!(
        out.len(),
        pool.len(),
        "every call path reachable from the root"
    );
    out
}

/// What the oracle knows about a span.
struct Model {
    callsite: Callsite,
    parent: Option<usize>,
    /// The call path, if recorded (within the depth limit).
    path: Option<Path>,
    dropped: bool,
    closed: bool,
    alive: u64,
    busy: u64,
    own: u64,
}

/// The expected sums, from the definition of the columns.
#[derive(Default)]
struct Oracle {
    /// All spans ever created, by key.
    models: Vec<Model>,
    /// The registry's stack of entered spans: (key, duplicate at push).
    stack: Vec<(usize, bool)>,
    /// The sums of the closed spans by root key and call path.
    expected: HashMap<usize, Sums>,
    /// The roots in the order they closed.
    roots_closed: Vec<usize>,
}

impl Oracle {
    /// The registry's current span: the last entry that is not a duplicate.
    fn current(&self) -> Option<usize> {
        self.stack
            .iter()
            .rev()
            .find(|(_, duplicate)| !duplicate)
            .map(|(key, _)| *key)
    }

    fn created(&mut self, callsite: Callsite, parent: Option<usize>) -> usize {
        let path = match parent {
            None => Some(vec![callsite]),
            Some(parent) => self.models[parent].path.as_ref().and_then(|parent_path| {
                (parent_path.len() < MAX_CALL_DEPTH).then(|| {
                    let mut path = parent_path.clone();
                    path.push(callsite);
                    path
                })
            }),
        };
        self.models.push(Model {
            callsite,
            parent,
            path,
            dropped: false,
            closed: false,
            alive: 0,
            busy: 0,
            own: 0,
        });
        self.models.len() - 1
    }

    fn entered(&mut self, key: usize) {
        let duplicate = self.stack.iter().any(|(k, _)| *k == key);
        self.stack.push((key, duplicate));
    }

    /// The registry pops the most recent entry of the span.
    fn exited(&mut self, key: usize) {
        let pos = self.stack.iter().rposition(|(k, _)| *k == key).unwrap();
        self.stack.remove(pos);
    }

    fn dropped(&mut self, key: usize) {
        self.models[key].dropped = true;
    }

    /// The clock moved on by `delta`: busy for every entered recorded span,
    /// own for the recorded span entered last (an unrecorded span on top is
    /// inlined into the recorded one below), alive for every open span.
    fn spend(&mut self, delta: u64) {
        let mut distinct: Vec<usize> = Vec::new();
        for (key, _) in &self.stack {
            if !distinct.contains(key) {
                distinct.push(*key);
            }
        }
        for key in distinct {
            if self.models[key].path.is_some() {
                self.models[key].busy += delta;
            }
        }
        if let Some((key, _)) = self
            .stack
            .iter()
            .rev()
            .find(|(key, _)| self.models[*key].path.is_some())
        {
            self.models[*key].own += delta;
        }
        for model in &mut self.models {
            if !model.closed && model.path.is_some() {
                model.alive += delta;
            }
        }
    }

    /// Closes every span whose handle is dropped, which is not entered and
    /// whose children are all closed; children first, so that a parent can
    /// close in the same step. Folds the closed spans into the sums.
    fn close_finished(&mut self) {
        loop {
            let mut closed_one = false;
            for key in 0..self.models.len() {
                if self.models[key].closed || !self.models[key].dropped {
                    continue;
                }
                if self.stack.iter().any(|(k, _)| *k == key) {
                    continue;
                }
                if self
                    .models
                    .iter()
                    .any(|m| m.parent == Some(key) && !m.closed)
                {
                    continue;
                }
                self.models[key].closed = true;
                closed_one = true;
                let Some(path) = self.models[key].path.clone() else {
                    continue;
                };
                let mut root = key;
                while let Some(parent) = self.models[root].parent {
                    root = parent;
                }
                let model = &self.models[key];
                let sums = self
                    .expected
                    .entry(root)
                    .or_default()
                    .entry(path)
                    .or_default();
                sums.0 += 1;
                sums.1 += u128::from(model.alive);
                sums.2 += u128::from(model.busy);
                sums.3 += u128::from(model.own);
                if root == key {
                    self.roots_closed.push(key);
                }
            }
            if !closed_one {
                break;
            }
        }
    }
}

/// The spans of a program on one thread, and the oracle following it.
#[derive(Default)]
struct Program {
    oracle: Oracle,
    live: Vec<(usize, Span)>,
    entered: Vec<(usize, EnteredSpan)>,
}

impl Program {
    fn step(&mut self, rng: &mut Rng, mock: &quanta::Mock, op: usize) {
        match op {
            // A new span: root, explicit parent or the current span.
            0..=2 => {
                let kind = rng.below(KINDS);
                let parent_key = match op {
                    1 => self.oracle.current(),
                    _ if self.live.is_empty() || (op == 0 && rng.chance(20)) => None,
                    _ => Some(self.live[rng.below(self.live.len())].0),
                };
                let (span, callsite) = match (op, parent_key) {
                    (1, _) => make_span(kind, Parent::Current),
                    (_, None) => make_span(kind, Parent::None),
                    (_, Some(key)) => {
                        let parent = &self.live.iter().find(|(k, _)| *k == key).unwrap().1;
                        make_span(kind, Parent::Explicit(parent))
                    }
                };
                let key = self.oracle.created(callsite, parent_key);
                self.live.push((key, span));
            }
            // Enter a live span, entered already or not.
            3..=5 if !self.live.is_empty() => {
                let (key, span) = &self.live[rng.below(self.live.len())];
                self.oracle.entered(*key);
                self.entered.push((*key, span.clone().entered()));
            }
            // Exit any entered span.
            6..=8 if !self.entered.is_empty() => {
                let (key, guard) = self.entered.remove(rng.below(self.entered.len()));
                self.oracle.exited(key);
                drop(guard);
            }
            // Drop the handle of a live span.
            9 if !self.live.is_empty() => {
                let (key, span) = self.live.remove(rng.below(self.live.len()));
                self.oracle.dropped(key);
                drop(span);
            }
            // The clock moves on.
            _ => {
                let delta = u64::try_from(rng.below(100) + 1).unwrap();
                mock.increment(delta);
                self.oracle.spend(delta);
            }
        }
        self.oracle.close_finished();
    }
}

/// A program on one thread, checked against the oracle.
fn single_thread_program(seed: u64, ops: usize) {
    let (clock, mock) = Clock::mock();
    let pools = Pools::default();
    let subscriber = tracing_subscriber::registry().with(
        CallTreeCollectorBuilder::default()
            .clock(clock)
            .max_call_depth(MAX_CALL_DEPTH)
            .build_with_collector(pools.clone()),
    );
    let mut rng = Rng(seed);
    let mut program = Program::default();

    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..ops {
            let op = rng.below(16);
            program.step(&mut rng, &mock, op);
        }
        // Exit and drop everything, in random order.
        while !program.entered.is_empty() {
            program.step(&mut rng, &mock, 6);
        }
        while !program.live.is_empty() {
            program.step(&mut rng, &mock, 9);
        }
    });

    let oracle = program.oracle;
    let pools = std::mem::take(&mut *pools.0.lock().unwrap());
    assert_eq!(
        pools.len(),
        oracle.roots_closed.len(),
        "seed {seed}: one pool per root"
    );
    for (pool, root) in pools.iter().zip(&oracle.roots_closed) {
        assert_eq!(callsite(pool.root()), oracle.models[*root].callsite);
        assert_eq!(
            sums(pool),
            oracle.expected[root],
            "seed {seed}, root {root}"
        );
    }
}

#[test]
fn single_thread_matches_oracle() {
    for seed in 1..=24 {
        single_thread_program(seed, 2_000);
    }
}

/// What the threads report: roots created by name, spans created.
#[derive(Default)]
struct Created {
    roots: Mutex<BTreeMap<&'static str, usize>>,
    spans: AtomicUsize,
}

/// A program on one of several threads: the same operations, with spans
/// shared between the threads and events for the `fmt` layer.
fn threaded_program(seed: u64, ops: usize, shared: &Mutex<Vec<Span>>, created: &Created) {
    let mut rng = Rng(seed);
    let mut live: Vec<Span> = Vec::new();
    let mut entered: Vec<EnteredSpan> = Vec::new();
    for _ in 0..ops {
        match rng.below(16) {
            0..=2 => {
                let kind = rng.below(KINDS);
                let shared_spans;
                let parent = match rng.below(100) {
                    _ if live.is_empty() => Parent::None,
                    0..15 => Parent::None,
                    15..40 => {
                        shared_spans = shared.lock().unwrap();
                        if shared_spans.is_empty() {
                            continue;
                        }
                        let index = rng.below(shared_spans.len());
                        Parent::Explicit(&shared_spans[index])
                    }
                    40..60 => Parent::Current,
                    _ => Parent::Explicit(&live[rng.below(live.len())]),
                };
                // A contextual span is a root when nothing is entered.
                let root = matches!(parent, Parent::None)
                    || (matches!(parent, Parent::Current) && Span::current().is_none());
                let (span, (name, _)) = make_span(kind, parent);
                if root {
                    *created.roots.lock().unwrap().entry(name).or_default() += 1;
                }
                created.spans.fetch_add(1, Ordering::Relaxed);
                live.push(span);
            }
            3..=5 => {
                let span = if !live.is_empty() && !rng.chance(30) {
                    live[rng.below(live.len())].clone()
                } else {
                    let shared = shared.lock().unwrap();
                    if shared.is_empty() {
                        continue;
                    }
                    let index = rng.below(shared.len());
                    shared[index].clone()
                };
                entered.push(span.entered());
            }
            6..=8 if !entered.is_empty() => {
                drop(entered.remove(rng.below(entered.len())));
            }
            9 if !live.is_empty() => {
                drop(live.remove(rng.below(live.len())));
            }
            10 if !live.is_empty() => {
                let mut shared = shared.lock().unwrap();
                if shared.len() < 16 {
                    let span = live[rng.below(live.len())].clone();
                    shared.push(span);
                } else {
                    let index = rng.below(shared.len());
                    shared.remove(index);
                }
            }
            11..=12 => info!(seed, "an event in the current span"),
            _ => {
                // Some real time passes.
                std::hint::black_box((0..rng.below(64)).sum::<usize>());
            }
        }
    }
}

#[test]
fn threads_keep_the_invariants() {
    const THREADS: u64 = 4;
    let pools = Pools::default();
    let subscriber = tracing_subscriber::registry()
        .with(
            CallTreeCollectorBuilder::default()
                .max_call_depth(MAX_CALL_DEPTH)
                .build_with_collector(pools.clone()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::sink),
        );
    let dispatch = Dispatch::new(subscriber);
    let shared = Arc::new(Mutex::new(Vec::new()));
    let created = Arc::new(Created::default());
    let workers: Vec<_> = (0..THREADS)
        .map(|index| {
            let dispatch = dispatch.clone();
            let shared = Arc::clone(&shared);
            let created = Arc::clone(&created);
            thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    threaded_program(1_000 + index, 4_000, &shared, &created);
                });
            })
        })
        .collect();
    // A deadlock would hang the test forever; fail it instead.
    let start = Instant::now();
    while !workers.iter().all(thread::JoinHandle::is_finished) {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the workers did not finish: a deadlock?"
        );
        thread::sleep(Duration::from_millis(5));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    // The registry closes a parent through the thread's default
    // dispatcher when the last child closes: drop the spans with it set.
    tracing::dispatcher::with_default(&dispatch, || shared.lock().unwrap().clear());
    drop(dispatch);

    let pools = std::mem::take(&mut *pools.0.lock().unwrap());
    let mut roots_reported: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut closed = 0;
    for pool in &pools {
        *roots_reported
            .entry(pool.root().static_span_meta().name())
            .or_default() += 1;
        for (path, (calls, alive, busy, _)) in sums(pool) {
            if path.len() == 1 {
                assert_eq!(calls, 1, "the root closes once");
            }
            closed += calls;
            // Busy on every thread at most.
            assert!(
                busy <= u128::from(THREADS) * alive,
                "{path:?}: busy {busy} alive {alive}"
            );
        }
    }
    assert_eq!(
        roots_reported,
        *created.roots.lock().unwrap(),
        "one pool per root, by name"
    );
    assert!(closed <= created.spans.load(Ordering::Relaxed));
}
