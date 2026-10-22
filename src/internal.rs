use std::{
    cell::RefCell,
    collections::HashMap,
    ops::{Index, IndexMut},
    time::Duration,
};

use thread_local::ThreadLocal;
use tracing::{Id, Metadata, Subscriber, callsite, span};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// Use a [`CallPathPoolId`] to index a [`CallPathTiming`] in a [`CallPathPool`].
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct CallPathPoolId(usize);

/// A [`CallPathPool`] contains all [`CallPathTiming`]s of a call tree
/// indexed by [`CallPathPoolId`]s.
///
/// The root is at index `0`; every other node comes after its parent.
#[derive(Debug)]
pub struct CallPathPool {
    timings: Vec<CallPathTiming>,
}

impl CallPathPool {
    /// The timing of the root span of the call tree.
    #[must_use]
    pub fn root(&self) -> &CallPathTiming {
        &self[CallPathPoolId(0)]
    }

    /// The number of distinct call paths in the tree.
    #[must_use]
    pub fn len(&self) -> usize {
        self.timings.len()
    }

    /// Whether the tree has no call paths. Never, for a finished tree.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.timings.is_empty()
    }

    /// All call paths with their ids, the root first and every node after
    /// its parent.
    pub fn iter(&self) -> impl Iterator<Item = (CallPathPoolId, &CallPathTiming)> {
        self.timings
            .iter()
            .enumerate()
            .map(|(idx, timing)| (CallPathPoolId(idx), timing))
    }
}

impl Index<CallPathPoolId> for CallPathPool {
    type Output = CallPathTiming;

    fn index(&self, CallPathPoolId(idx): CallPathPoolId) -> &Self::Output {
        &self.timings[idx]
    }
}

impl IndexMut<CallPathPoolId> for CallPathPool {
    fn index_mut(&mut self, CallPathPoolId(idx): CallPathPoolId) -> &mut Self::Output {
        &mut self.timings[idx]
    }
}

/// A `CallPathTiming` is an aggregation of all spans with the same
/// call path. That means that their `callsite::Identifier` is
/// the same and all the `callsite::Identifier`s of their ancestor
/// spans are also the same.
#[derive(Debug, Clone)]
pub struct CallPathTiming {
    depth: usize,
    call_count: usize,
    span_meta: &'static Metadata<'static>,
    children: HashMap<callsite::Identifier, CallPathPoolId>,
    span_life_time: Duration,
    sum_with_children: Duration,
    sum_own: Duration,
}

impl CallPathTiming {
    fn new(depth: usize, span_meta: &'static Metadata<'static>) -> Self {
        CallPathTiming {
            depth,
            call_count: 0,
            span_meta,
            children: HashMap::new(),
            span_life_time: Duration::ZERO,
            sum_with_children: Duration::ZERO,
            sum_own: Duration::ZERO,
        }
    }

    /// The metadata associated with the called instrumented span,
    /// includes e.g. the name of the function that is being executed.
    #[must_use]
    pub fn static_span_meta(&self) -> &'static Metadata<'static> {
        self.span_meta
    }

    /// The depth in the call tree: `0` for the root, `1` for its children
    /// and so on.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// The number of times a span with this call path was closed.
    ///
    /// Typically, the number of times a function was called.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.call_count
    }

    /// The sum of the durations between the creation and the close of
    /// spans with this call path.
    #[must_use]
    pub fn span_alive(&self) -> Duration {
        self.span_life_time
    }

    /// The total sum of the durations between entering and exiting spans
    /// with this call path, "busy" time. The time spent in sub spans is
    /// included.
    ///
    /// A span entered on several threads at once is busy on each of them,
    /// so the sum can exceed [`Self::span_alive`]. Entering a span again
    /// on a thread where it is already entered does not count twice.
    #[must_use]
    pub fn sum_with_children(&self) -> Duration {
        self.sum_with_children
    }

    /// The total sum of the durations during which a span with this call
    /// path was the innermost entered span of a thread, "own" time: busy
    /// time minus the time another span was entered on top of it.
    ///
    /// The innermost entered span is the one entered last, also when that
    /// is the span itself entered again (`tracing_subscriber`'s registry
    /// skips such an entry for `Span::current()`; here it keeps the own
    /// time with the span, so that the own times of a tree add up).
    ///
    /// Spans that are not recorded (beyond the maximum call depth) do not
    /// count as entered, so their time goes to the span they are inlined
    /// into.
    #[must_use]
    pub fn sum_without_children(&self) -> Duration {
        self.sum_own
    }

    /// An iterator over the IDs of all children, in no particular order.
    pub fn children(&self) -> impl Iterator<Item = &CallPathPoolId> {
        self.children.values()
    }
}

/// The span specific information, in the span's extensions.
///
/// The sums are folded into the referenced [`CallPathTiming`] when
/// the span is closed.
#[derive(Debug)]
struct SpanTimingInfo {
    call_path_idx: CallPathPoolId,
    /// The root of the span's tree, whose extensions hold the
    /// [`CallPathPool`]. The span itself for a root.
    root: Id,
    /// The clock reading at which the span was created.
    created_at: u64,
    sum_busy: Duration,
    sum_own: Duration,
}

/// An entry of a thread's stack of entered spans.
#[derive(Debug)]
struct Entered {
    id: u64,
    /// The clock reading when the span was entered.
    started_at: u64,
    /// The clock reading since which the span has been the innermost
    /// entered span of the thread. `None` unless it is the top of the stack.
    own_since: Option<u64>,
    /// Own time accrued so far while the span was the top of the stack.
    own: Duration,
    /// The span was already entered on this thread when it was entered
    /// again (e.g. a future instrumented with the current span): its busy
    /// time is covered by the earlier entry.
    duplicate: bool,
}

/// The stack of entered spans of one thread. Spans that are not recorded
/// (beyond the maximum call depth) are not on it.
#[derive(Debug, Default)]
pub(crate) struct EnteredStack {
    entries: RefCell<Vec<Entered>>,
}

/// Per-thread stacks of entered spans, owned by a [`crate::CallTreeCollector`].
///
/// `thread_local` recycles thread ids, so a new thread can inherit the
/// stack of an exited one. It is empty unless a guard was leaked on that
/// thread (`mem::forget`), and the registry's own stack of current spans
/// behaves the same.
pub(crate) type EnteredStacks = ThreadLocal<EnteredStack>;

// Implementation idea:
//
// Each recorded span has a [SpanTimingInfo] in its extensions. In
// parallel, we build an aggregated hierarchy for every call path of
// [CallPathTiming]: a [CallPathPool] in the extensions of the root span
// owns all [CallPathTiming]s of its tree. Whenever a span is closed, we
// fold its sums into the corresponding [CallPathTiming].
//
// Entering and exiting only touches a per-thread stack of the entered
// spans and, on exit, the span's own extensions: no lookups in the pool.
// This matters in async code, where a span is entered and exited at every
// poll. The stack also makes the accounting exact however spans are
// entered: nested again on the same thread, on several threads at once,
// on top of a span that is not their parent, or exited in another order
// than entered (the registry allows all of this).
impl<S, H> Layer<S> for crate::CallTreeCollector<H>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    H: crate::FinishedCallTreeProcessor + 'static,
{
    fn on_new_span(&self, _attrs: &span::Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let created_at = self.clock.raw();
        let Some(parent) = span.parent() else {
            let mut extensions = span.extensions_mut();
            extensions.insert(CallPathPool {
                timings: vec![CallPathTiming::new(0, span.metadata())],
            });
            extensions.insert(SpanTimingInfo {
                call_path_idx: CallPathPoolId(0),
                root: id.clone(),
                created_at,
                sum_busy: Duration::ZERO,
                sum_own: Duration::ZERO,
            });
            return;
        };

        // Never hold the extensions of two spans at once.
        let (parent_call_path_idx, root_id) = {
            let parent_extensions = parent.extensions();
            let Some(info) = parent_extensions.get::<SpanTimingInfo>() else {
                // The parent is beyond the maximum call depth, and so are we.
                return;
            };
            (info.call_path_idx, info.root.clone())
        };
        // The root is alive: the registry keeps the ancestors of a span
        // until the span is closed.
        let Some(root) = ctx.span(&root_id) else {
            return;
        };
        let call_path_idx = {
            let mut root_extensions = root.extensions_mut();
            let Some(pool) = root_extensions.get_mut::<CallPathPool>() else {
                return;
            };
            let new_idx = CallPathPoolId(pool.timings.len());
            let parent_call_path_timing = &mut pool[parent_call_path_idx];
            let new_depth = parent_call_path_timing.depth + 1;
            if new_depth >= self.max_call_depth {
                // Not recorded: inlined into the parent.
                return;
            }
            let callsite = span.metadata().callsite();
            if let Some(idx) = parent_call_path_timing.children.get(&callsite) {
                *idx
            } else {
                parent_call_path_timing.children.insert(callsite, new_idx);
                pool.timings
                    .push(CallPathTiming::new(new_depth, span.metadata()));
                new_idx
            }
        };
        span.extensions_mut().insert(SpanTimingInfo {
            call_path_idx,
            root: root_id,
            created_at,
            sum_busy: Duration::ZERO,
            sum_own: Duration::ZERO,
        });
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if span.extensions().get::<SpanTimingInfo>().is_none() {
            // Beyond the maximum call depth: inlined into the span below.
            return;
        }
        let stack = self.entered.get_or_default();
        let mut entries = stack.entries.borrow_mut();
        let id = id.into_u64();
        let duplicate = entries.iter().any(|entry| entry.id == id);
        let now = self.clock.raw();
        if let Some(top) = entries.last_mut()
            && let Some(since) = top.own_since.take()
        {
            top.own += self.elapsed(since, now);
        }
        entries.push(Entered {
            id,
            started_at: now,
            own_since: Some(now),
            own: Duration::ZERO,
            duplicate,
        });
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let now = self.clock.raw();
        let Some(stack) = self.entered.get() else {
            return;
        };
        let entry = {
            let mut entries = stack.entries.borrow_mut();
            // The most recent entry, as the registry pops it.
            let Some(pos) = entries.iter().rposition(|entry| entry.id == id.into_u64()) else {
                // Not recorded, or entered on another thread.
                return;
            };
            let mut entry = entries.remove(pos);
            if let Some(since) = entry.own_since.take() {
                // It was the top: the span below it is the innermost again.
                entry.own += self.elapsed(since, now);
                if let Some(top) = entries.last_mut() {
                    top.own_since = Some(now);
                }
            }
            entry
        };

        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        if let Some(info) = extensions.get_mut::<SpanTimingInfo>() {
            info.sum_own += entry.own;
            if !entry.duplicate {
                info.sum_busy += self.elapsed(entry.started_at, now);
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let closed = self.clock.raw();
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(info) = span.extensions_mut().remove::<SpanTimingInfo>() else {
            return;
        };
        // Children keep their parents alive, so the root is still there
        // and its pool is complete once the root itself closes.
        let Some(root) = ctx.span(&info.root) else {
            return;
        };
        let finished = {
            let mut root_extensions = root.extensions_mut();
            let Some(pool) = root_extensions.get_mut::<CallPathPool>() else {
                return;
            };
            let call_path_timing = &mut pool[info.call_path_idx];
            call_path_timing.call_count += 1;
            call_path_timing.span_life_time += self.elapsed(info.created_at, closed);
            call_path_timing.sum_with_children += info.sum_busy;
            call_path_timing.sum_own += info.sum_own;
            if info.root == id {
                root_extensions.remove::<CallPathPool>()
            } else {
                None
            }
        };
        // Without any extensions held: the processor may log, which other
        // layers format by reading span extensions.
        if let Some(pool) = finished {
            self.processor.process_finished_call(pool);
        }
    }
}

impl<H: crate::FinishedCallTreeProcessor + 'static> crate::CallTreeCollector<H> {
    /// The duration between two clock readings, zero if the clock went
    /// backwards (a counter read on another core).
    fn elapsed(&self, start: u64, end: u64) -> Duration {
        if end >= start {
            self.clock.delta(start, end)
        } else {
            Duration::ZERO
        }
    }
}

#[cfg(test)]
pub(crate) mod test {
    use std::{
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Duration,
    };

    use futures::channel::mpsc::{Receiver, Sender, channel};
    use quanta::{Clock, Mock};
    use tracing::{Instrument, Span, info, info_span, instrument::WithSubscriber};
    use tracing_subscriber::fmt;

    use crate::{
        CallPathPool, CallPathTiming, CallTreeCollectorBuilder, FinishedCallTreeProcessor,
    };

    #[tracing::instrument]
    pub(crate) fn one_ns(mock: &Mock) {
        mock.increment(1);
    }

    #[test]
    fn test_simple() {
        let call_trees = collect_call_trees(|mock| {
            one_ns(&mock);
        });

        assert_eq!(call_trees.len(), 1, "{call_trees:#?}");

        let first_call = &call_trees[0];
        assert_eq!(first_call.len(), 1, "{first_call:#?}");
        let first_call_root = first_call.root();
        assert_eq!(first_call_root.static_span_meta().name(), "one_ns");
        assert_eq!(first_call_root.depth(), 0);
        assert_eq!(first_call_root.call_count(), 1);
        assert_eq!(first_call_root.span_alive(), Duration::from_nanos(1));
        assert_eq!(first_call_root.sum_with_children(), Duration::from_nanos(1));
        assert_eq!(
            first_call_root.sum_without_children(),
            Duration::from_nanos(1)
        );
    }

    #[tracing::instrument]
    pub(crate) fn compound_call(mock: &Mock) {
        mock.increment(10);
        one_ns(mock);
        mock.increment(100);
        one_ns(mock);
        one_ns(mock);
        mock.increment(1000);
    }

    #[test]
    fn test_compound() {
        let call_trees = collect_call_trees(|mock| {
            compound_call(&mock);
        });

        assert_eq!(call_trees.len(), 1, "{call_trees:#?}");

        let first_call = &call_trees[0];
        assert_eq!(first_call.len(), 2, "{first_call:#?}");

        let first_call_root = first_call.root();
        assert_eq!(first_call_root.static_span_meta().name(), "compound_call");
        assert_eq!(first_call_root.call_count(), 1);
        assert_eq!(
            first_call_root.sum_with_children(),
            Duration::from_nanos(1113)
        );
        assert_eq!(
            first_call_root.sum_without_children(),
            Duration::from_nanos(1110)
        );
        assert_eq!(first_call_root.children().count(), 1);

        let nested_call_idx = *first_call_root.children().next().unwrap();
        let nested_call = &first_call[nested_call_idx];
        assert_eq!(nested_call.static_span_meta().name(), "one_ns");
        assert_eq!(nested_call.depth(), 1);
        assert_eq!(nested_call.call_count(), 3);
        assert_eq!(nested_call.span_alive(), Duration::from_nanos(3));
        assert_eq!(nested_call.sum_with_children(), Duration::from_nanos(3));
        assert_eq!(nested_call.sum_without_children(), Duration::from_nanos(3));

        let by_id: Vec<_> = first_call
            .iter()
            .map(|(_, t)| t.static_span_meta().name())
            .collect();
        assert_eq!(by_id, ["compound_call", "one_ns"]);
    }

    /// The timing of the call path named `name`, which must be unique.
    pub(crate) fn timing<'a>(pool: &'a CallPathPool, name: &str) -> &'a CallPathTiming {
        let mut found = pool
            .iter()
            .filter(|(_, t)| t.static_span_meta().name() == name);
        let (_, timing) = found
            .next()
            .unwrap_or_else(|| panic!("no span {name} in {pool:#?}"));
        assert!(found.next().is_none(), "several spans {name} in {pool:#?}");
        timing
    }

    fn assert_timing(timing: &CallPathTiming, calls: usize, busy: u64, own: u64) {
        assert_eq!(
            (
                timing.call_count(),
                timing.sum_with_children(),
                timing.sum_without_children()
            ),
            (calls, Duration::from_nanos(busy), Duration::from_nanos(own)),
            "(calls, busy, own) of {}",
            timing.static_span_meta().name()
        );
    }

    /// Entering a span again on a thread where it is entered already, e.g.
    /// a future instrumented with the current span and awaited inside it,
    /// does not count its busy time twice, and all of its time is its own.
    #[test]
    fn reentered_span_counts_once() {
        let call_trees = collect_call_trees(|mock| {
            let root = info_span!("root");
            let root_guard = root.enter();
            mock.increment(1);
            let child = info_span!("child");
            let outer = child.enter();
            mock.increment(2);
            let inner = child.enter();
            mock.increment(4);
            drop(inner);
            mock.increment(8);
            drop(outer);
            mock.increment(16);
            drop(root_guard);
        });
        let tree = &call_trees[0];
        assert_timing(timing(tree, "root"), 1, 31, 17);
        assert_timing(timing(tree, "child"), 1, 14, 14);
    }

    /// Two children of the same parent entered at the same time: the one
    /// entered last owns the time, its sibling and the parent wait.
    #[test]
    fn siblings_entered_at_once() {
        let call_trees = collect_call_trees(|mock| {
            let root = info_span!("root");
            let root_guard = root.enter();
            let a = info_span!("a");
            let b = info_span!("b");
            mock.increment(1);
            let a_guard = a.enter();
            mock.increment(2);
            let b_guard = b.enter();
            mock.increment(4);
            drop(b_guard);
            mock.increment(8);
            drop(a_guard);
            mock.increment(16);
            drop(root_guard);
        });
        let tree = &call_trees[0];
        assert_eq!(tree.len(), 3, "{tree:#?}");
        assert_eq!(timing(tree, "a").depth(), 1);
        assert_eq!(timing(tree, "b").depth(), 1);
        assert_timing(timing(tree, "root"), 1, 31, 17);
        assert_timing(timing(tree, "a"), 1, 14, 10);
        assert_timing(timing(tree, "b"), 1, 4, 4);
    }

    /// Guards dropped in another order than entered: the span exited first
    /// stops being busy, the span on top of it keeps the own time.
    #[test]
    fn guards_dropped_out_of_order() {
        let call_trees = collect_call_trees(|mock| {
            let root = info_span!("root");
            let root_guard = root.enter();
            mock.increment(1);
            let a = info_span!("a");
            let a_guard = a.enter();
            mock.increment(2);
            let b = info_span!("b");
            let b_guard = b.enter();
            mock.increment(4);
            drop(a_guard);
            mock.increment(8);
            drop(b_guard);
            mock.increment(16);
            drop(root_guard);
        });
        let tree = &call_trees[0];
        assert_eq!(timing(tree, "b").depth(), 2);
        assert_timing(timing(tree, "root"), 1, 31, 17);
        assert_timing(timing(tree, "a"), 1, 6, 2);
        assert_timing(timing(tree, "b"), 1, 12, 12);
    }

    /// Spans beyond the maximum call depth are inlined into the deepest
    /// recorded span: not counted, their time is its own time.
    #[test]
    fn beyond_max_depth_is_inlined() {
        let call_trees = collect_call_trees_with(2, |mock| {
            let root = info_span!("root");
            let root_guard = root.enter();
            mock.increment(1);
            let a = info_span!("a");
            let a_guard = a.enter();
            mock.increment(2);
            let b = info_span!("b");
            let b_guard = b.enter();
            mock.increment(4);
            let c = info_span!("c");
            let c_guard = c.enter();
            mock.increment(8);
            drop(c_guard);
            drop(b_guard);
            mock.increment(16);
            drop(a_guard);
            mock.increment(32);
            drop(root_guard);
        });
        let tree = &call_trees[0];
        let names: Vec<_> = tree
            .iter()
            .map(|(_, t)| t.static_span_meta().name())
            .collect();
        assert_eq!(names, ["root", "a"]);
        assert_timing(timing(tree, "root"), 1, 63, 33);
        assert_timing(timing(tree, "a"), 1, 30, 30);
    }

    /// A span entered on two threads at once is busy on both; a thread's
    /// own time goes to the span entered last on that thread.
    #[test]
    fn entered_on_several_threads() {
        let call_trees = collect_call_trees(|mock| {
            let root = info_span!("root");
            let root_guard = root.enter();
            let child = info_span!("child");
            let (to_other, other_inbox) = mpsc::channel::<()>();
            let (to_main, main_inbox) = mpsc::channel::<()>();
            let other = thread::spawn({
                let child = child.clone();
                move || {
                    other_inbox.recv().unwrap();
                    let guard = child.enter();
                    to_main.send(()).unwrap();
                    other_inbox.recv().unwrap();
                    drop(guard);
                    drop(child);
                    to_main.send(()).unwrap();
                }
            });
            mock.increment(1);
            to_other.send(()).unwrap();
            main_inbox.recv().unwrap();
            mock.increment(2);
            let guard = child.enter();
            mock.increment(4);
            drop(guard);
            mock.increment(8);
            to_other.send(()).unwrap();
            main_inbox.recv().unwrap();
            other.join().unwrap();
            mock.increment(16);
            drop(root_guard);
        });
        let tree = &call_trees[0];
        assert_timing(timing(tree, "root"), 1, 31, 27);
        // On the other thread from 1 to 15, on the main thread from 3 to 7.
        assert_timing(timing(tree, "child"), 1, 18, 18);
    }

    /// A span created on another thread with an explicit parent joins the
    /// parent's tree.
    #[test]
    fn explicit_parent_from_another_thread() {
        let call_trees = collect_call_trees(|mock| {
            let root = info_span!("root");
            let root_guard = root.enter();
            mock.increment(1);
            // The subscriber is the default of the test's thread alone.
            let dispatch = tracing::dispatcher::get_default(Clone::clone);
            thread::spawn({
                let root = root.clone();
                let mock = mock.clone();
                move || {
                    tracing::dispatcher::with_default(&dispatch, || {
                        let child = info_span!(parent: &root, "child");
                        let guard = child.enter();
                        mock.increment(2);
                        drop(guard);
                    });
                }
            })
            .join()
            .unwrap();
            mock.increment(4);
            drop(root_guard);
        });
        let tree = &call_trees[0];
        assert_eq!(tree.len(), 2);
        assert_eq!(timing(tree, "child").depth(), 1);
        // The main thread never left the root.
        assert_timing(timing(tree, "root"), 1, 7, 7);
        assert_timing(timing(tree, "child"), 1, 2, 2);
    }

    /// A span created without a parent while another root is entered is a
    /// tree of its own, but takes the own time of the thread while entered.
    #[test]
    fn root_entered_on_top_of_another_root() {
        let call_trees = collect_call_trees(|mock| {
            let first = info_span!("first");
            let first_guard = first.enter();
            mock.increment(1);
            let second = info_span!(parent: None, "second");
            let second_guard = second.enter();
            mock.increment(2);
            drop(second_guard);
            drop(second);
            mock.increment(4);
            drop(first_guard);
        });
        assert_eq!(call_trees.len(), 2, "{call_trees:#?}");
        let second = &call_trees[0];
        assert_eq!(second.len(), 1);
        assert_timing(second.root(), 1, 2, 2);
        let first = &call_trees[1];
        assert_eq!(first.len(), 1);
        assert_timing(first.root(), 1, 7, 5);
    }

    /// A span that is never entered still counts as a call with its
    /// alive time.
    #[test]
    fn never_entered_span() {
        let call_trees = collect_call_trees(|mock| {
            let root = info_span!("root");
            let root_guard = root.enter();
            let child = info_span!("child");
            mock.increment(3);
            drop(child);
            mock.increment(4);
            drop(root_guard);
        });
        let tree = &call_trees[0];
        let child = timing(tree, "child");
        assert_timing(child, 1, 0, 0);
        assert_eq!(child.span_alive(), Duration::from_nanos(3));
        assert_timing(timing(tree, "root"), 1, 7, 7);
    }

    #[tracing::instrument(skip(mock, receiver))]
    pub(crate) async fn eat_three(mock: Arc<Mock>, mut receiver: Receiver<usize>) {
        use futures::StreamExt;
        for _ in 0..3 {
            let _next = receiver.next().await.unwrap();
            info!("increment 1_000");
            mock.increment(1_000);
        }
    }

    #[tracing::instrument(skip(mock, sender))]
    pub(crate) async fn cook_three(mock: Arc<Mock>, mut sender: Sender<usize>) {
        use futures::SinkExt;
        for _ in 0..3 {
            info!("increment 10_000");
            mock.increment(10_000);
            sender.send(0).await.unwrap();
        }
    }

    #[tracing::instrument(skip(mock))]
    pub(crate) async fn cooking_party(mock: Arc<Mock>) {
        // Use "no" buffer (which means a buffer of one for each sender)
        // to enforce a deterministic order.
        let (sender, receiver) = channel(0);
        info!("CP increment 1_000_000");
        mock.increment(1_000_000);

        let handle = tokio::spawn({
            let mock = mock.clone();
            async {
                eat_three(mock, receiver).await;
            }
            .in_current_span()
            // A worker thread of a multi-thread runtime has no default
            // subscriber; the spans created there need this one.
            .with_current_subscriber()
        });
        // Let the task create its span before the clock moves on, so that
        // its alive time does not depend on when the runtime first polls it.
        tokio::task::yield_now().await;
        cook_three(mock.clone(), sender).await;

        handle.await.unwrap();
        info!("CP increment 100_000_000");
        mock.increment(100_000_000);
    }

    /// Runs `future` on a runtime with one thread, so that the clock
    /// increments of the tasks are deterministic.
    pub(crate) fn block_on_single_thread<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// Tasks entering the same span by turns on one thread (`in_current_span`),
    /// with spans of their own below it.
    #[test]
    fn test_with_futures() {
        let call_trees = collect_call_trees(|mock| {
            block_on_single_thread(cooking_party(mock));
        });
        assert_eq!(call_trees.len(), 1, "{call_trees:#?}");
        let tree = &call_trees[0];
        let party = timing(tree, "cooking_party");
        let cook = timing(tree, "cook_three");
        let eat = timing(tree, "eat_three");
        assert_eq!(party.depth(), 0);
        assert_eq!(cook.depth(), 1);
        assert_eq!(eat.depth(), 1);
        assert_eq!(cook.call_count(), 1);
        assert_eq!(eat.call_count(), 1);
        let total = Duration::from_micros(101_033);
        assert_eq!(party.sum_with_children(), total, "{tree:#?}");
        assert_eq!(party.span_alive(), total, "{tree:#?}");
        assert_eq!(
            cook.sum_with_children(),
            Duration::from_micros(30),
            "{tree:#?}"
        );
        assert_eq!(cook.sum_without_children(), Duration::from_micros(30));
        assert_eq!(
            eat.sum_with_children(),
            Duration::from_micros(3),
            "{tree:#?}"
        );
        assert_eq!(eat.sum_without_children(), Duration::from_micros(3));
        // On one thread, the own times add up to the root's busy time.
        let own_sum: Duration = tree.iter().map(|(_, t)| t.sum_without_children()).sum();
        assert_eq!(own_sum, total);
        assert_eq!(party.sum_without_children(), Duration::from_millis(101));
    }

    /// A span is current across a `tokio::spawn` boundary, on a runtime
    /// with several threads: the tree is the same. The threads share the
    /// mock clock, so an increment on one thread also counts for a span
    /// entered on the other at that moment: only lower bounds are exact.
    #[test]
    fn test_with_futures_multi_thread() {
        let call_trees = collect_call_trees(|mock| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .build()
                .unwrap();
            // The subscriber is the thread's default; a worker needs it too.
            let dispatch = tracing::dispatcher::get_default(Clone::clone);
            runtime.block_on(async move {
                let party = tracing::dispatcher::with_default(&dispatch, || cooking_party(mock));
                party.with_subscriber(dispatch).await;
            });
        });
        assert_eq!(call_trees.len(), 1, "{call_trees:#?}");
        let tree = &call_trees[0];
        let names: Vec<_> = tree
            .iter()
            .map(|(_, t)| t.static_span_meta().name())
            .collect();
        assert_eq!(names.len(), 3);
        assert_eq!(names[0], "cooking_party");
        assert!(
            names.contains(&"cook_three") && names.contains(&"eat_three"),
            "{names:?}"
        );
        for (_, t) in tree.iter() {
            assert_eq!(t.call_count(), 1, "{tree:#?}");
            assert!(
                t.sum_without_children() <= t.sum_with_children(),
                "{tree:#?}"
            );
        }
        let party = timing(tree, "cooking_party");
        assert_eq!(party.span_alive(), Duration::from_micros(101_033));
        // Its own increments on the main thread, plus eat_three's on the
        // worker, where it is entered through `in_current_span`.
        assert!(
            party.sum_with_children() >= Duration::from_micros(101_033),
            "{tree:#?}"
        );
        assert!(
            party.sum_without_children() >= Duration::from_millis(101),
            "{tree:#?}"
        );
        assert!(timing(tree, "cook_three").sum_without_children() >= Duration::from_micros(30));
        assert!(timing(tree, "eat_three").sum_without_children() >= Duration::from_micros(3));
    }

    pub(crate) fn collect_call_trees(call: impl FnOnce(Arc<Mock>)) -> Vec<CallPathPool> {
        collect_call_trees_with(10, call)
    }

    pub(crate) fn collect_call_trees_with(
        max_call_depth: usize,
        call: impl FnOnce(Arc<Mock>),
    ) -> Vec<CallPathPool> {
        use tracing_subscriber::prelude::*;

        let call_trees = FinishedCallTreeStore::default();
        {
            let (clock, mock) = Clock::mock();
            let call_tree_collector = CallTreeCollectorBuilder::default()
                .clock(clock)
                .max_call_depth(max_call_depth)
                .build_with_collector(call_trees.clone());
            let fmt_layer = fmt::layer()
                .with_thread_ids(true)
                .without_time()
                .with_target(false)
                .with_test_writer();
            let subscriber = tracing_subscriber::registry()
                .with(call_tree_collector)
                .with(fmt_layer);
            tracing::subscriber::with_default(subscriber, || {
                call(mock);
            });
        }
        call_trees.into_vec()
    }

    #[derive(Clone, Default)]
    pub(crate) struct FinishedCallTreeStore {
        store: Arc<Mutex<Vec<CallPathPool>>>,
    }

    impl FinishedCallTreeStore {
        /// The trees finished so far. Not `Arc::try_unwrap`: `tracing` keeps
        /// a weak reference to every dispatcher and upgrades it for a moment
        /// whenever a callsite is registered, also from other threads.
        pub(crate) fn into_vec(self) -> Vec<CallPathPool> {
            std::mem::take(&mut *self.store.lock().unwrap())
        }
    }

    impl FinishedCallTreeProcessor for FinishedCallTreeStore {
        fn process_finished_call(&self, pool: CallPathPool) {
            let mut guard = self.store.lock().expect("getting collect log");
            guard.push(pool);
        }
    }

    /// `Span::current()` sees the registry's current span, which the layer
    /// follows for spans entered once: a span entered on top of an
    /// unrelated one is current (for a span entered again, see
    /// [`CallPathTiming::sum_without_children`]).
    #[test]
    fn current_span_matches_registry() {
        collect_call_trees(|_| {
            let root = info_span!("root");
            let _root_guard = root.enter();
            let other = info_span!(parent: None, "other");
            let _other_guard = other.enter();
            assert_eq!(Span::current().id(), other.id());
        });
    }
}
