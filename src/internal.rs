use std::{collections::HashMap, fmt, thread::ThreadId, time::Duration};
use tracing::{span::Attributes, Id, Subscriber};
use tracing_subscriber::{
    layer::Context,
    registry::{ExtensionsMut, LookupSpan},
    Layer,
};

use std::ops::{Index, IndexMut};

use tracing::{callsite, Metadata};

/// Use a [CallPathPoolId] to index a [CallPathTiming] in a [CallPathPool].
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct CallPathPoolId(usize);

/// A [CallPathPool] contains all [CallPathTiming]s of a call tree
/// indexed by [CallPathPoolId]s.
#[derive(Debug)]
pub struct CallPathPool {
    pool: Vec<CallPathTiming>,
}

impl CallPathPool {
    pub fn root(&self) -> &CallPathTiming {
        &self[CallPathPoolId(0)]
    }
}

impl Index<CallPathPoolId> for CallPathPool {
    type Output = CallPathTiming;

    fn index(&self, CallPathPoolId(idx): CallPathPoolId) -> &Self::Output {
        &self.pool[idx]
    }
}

impl IndexMut<CallPathPoolId> for CallPathPool {
    fn index_mut(&mut self, CallPathPoolId(idx): CallPathPoolId) -> &mut Self::Output {
        &mut self.pool[idx]
    }
}

/// A CallPathTiming is an aggregation of all spans with the same
/// call path. That means that their `callsite::Identifier` is
/// the same and all the `callsite::Identifier`s of their ancestor
/// spans are also the same.
#[derive(Debug, Clone)]
pub struct CallPathTiming {
    parent_idx: Option<CallPathPoolId>,
    depth: usize,
    call_count: usize,
    span_meta: &'static Metadata<'static>,
    children: HashMap<callsite::Identifier, CallPathPoolId>,
    sum_with_children: Duration,
    sum_own: Duration,
}

impl CallPathTiming {
    /// The metadata associated with the called instrumented span,
    /// includes e.g. the name of the function that is being executed.
    pub fn static_span_meta(&self) -> &'static Metadata<'static> {
        self.span_meta
    }

    /// The number of times a new span with this call path was created.
    ///
    /// Typically, the number of times a function was called.
    pub fn call_count(&self) -> usize {
        self.call_count
    }

    /// The total sum of durations between entering and leaving spans
    /// with this call path. The time spent in sub spans is included.
    pub fn sum_with_children(&self) -> Duration {
        self.sum_with_children
    }

    /// The total sum of durations between entering and leaving spans
    /// with this call path but the durations where we entered a sub
    /// span are excluded.
    pub fn sum_without_children(&self) -> Duration {
        self.sum_own
    }

    /// An iterator over the IDs of all children.
    pub fn children(&self) -> impl Iterator<Item = &CallPathPoolId> {
        self.children.values()
    }
}

/// The span specific information.
///
/// The sums are folded into the referenced [CallPathTiming] when
/// the span is closed.
#[derive(Debug, Clone)]
struct SpanTimingInfo {
    call_path_idx: CallPathPoolId,
    sum_with_children: Duration,
    sum_own: Duration,
    /// Per thread info. We always access SpanTimingInfo in a thread-safe way
    /// but we still need to keep some info per-thread:
    /// While not typical, the same span can be entered multiple times from multiple threads.
    per_thread: HashMap<ThreadId, PerThreadInfo>,
}

#[derive(Debug, Clone, Default)]
struct PerThreadInfo {
    last_enter: u64,
    last_enter_own: u64,
}

impl SpanTimingInfo {
    fn for_call_path_idx(call_path_idx: CallPathPoolId) -> SpanTimingInfo {
        SpanTimingInfo {
            call_path_idx,
            sum_with_children: Duration::default(),
            sum_own: Duration::default(),
            per_thread: HashMap::new(),
        }
    }
}

// Implementation idea:
//
// Each Span has a [SpanTimingInfo]. In parallel, we build
// an aggregated hierarchy for every call path of [CallPathTiming].
// We have a [CallPathPool] owning all [CallPathTiming]s at the root span.
// Whenever a Span is closed, we fold its aggregation values in
// the corresponding [CallPathTiming].
//
// This way, when entering/leaving a span, we only touch the
// span specific data without fancy lookups. This is important
// in async code where a span might be entered/left many times.
impl<S, H> Layer<S> for crate::CallTreeCollector<H>
where
    S: Subscriber + for<'span> LookupSpan<'span> + fmt::Debug,
    H: crate::FinishedCallTreeProcessor + 'static,
{
    fn new_span(&self, _attrs: &Attributes, id: &Id, ctx: Context<S>) {
        let span = ctx.span(id).expect("no span in new_span");
        match span.parent() {
            None => {
                // root
                let pool = vec![CallPathTiming {
                    parent_idx: None,
                    depth: 0,
                    call_count: 0,
                    span_meta: span.metadata(),
                    children: HashMap::new(),
                    sum_with_children: Duration::default(),
                    sum_own: Duration::default(),
                }];
                let mut extensions: ExtensionsMut = span.extensions_mut();
                extensions.insert(CallPathPool { pool });
                extensions.insert(SpanTimingInfo::for_call_path_idx(CallPathPoolId(0)));
            }
            Some(parent) => {
                let mut parent_extensions = parent.extensions_mut();
                let parent_span_info = parent_extensions.get_mut::<SpanTimingInfo>();
                if parent_span_info.is_none() {
                    // We are beyond the maximum tracing depth.
                    return;
                }

                let parent_call_path_idx = parent_span_info
                    .expect("parent has no SpanTimingInfo")
                    .call_path_idx;
                let root = span
                    .from_root()
                    .next()
                    .expect("span has a parent but no root");
                let mut root_extensions: ExtensionsMut = if root.id() == parent.id() {
                    parent_extensions
                } else {
                    // Do not keep multiple extensions locked at the same time.
                    std::mem::drop(parent_extensions);
                    root.extensions_mut()
                };
                let pool: &mut CallPathPool = root_extensions.get_mut::<CallPathPool>().unwrap();
                let new_idx = CallPathPoolId(pool.pool.len());
                let parent_call_path_timing = &mut pool[parent_call_path_idx];
                let new_depth = parent_call_path_timing.depth + 1;
                if new_depth >= self.max_call_depth {
                    return;
                }
                let idx = parent_call_path_timing
                    .children
                    .get(&span.metadata().callsite());
                let call_path_idx = match idx {
                    Some(idx) => *idx,
                    None => {
                        parent_call_path_timing
                            .children
                            .insert(span.metadata().callsite(), new_idx);
                        pool.pool.push(CallPathTiming {
                            parent_idx: Some(parent_call_path_idx),
                            depth: new_depth,
                            call_count: 0,
                            span_meta: span.metadata(),
                            children: HashMap::new(),
                            sum_with_children: Duration::default(),
                            sum_own: Duration::default(),
                        });
                        new_idx
                    }
                };
                // Do not keep multiple extensions locked at the same time.
                std::mem::drop(root_extensions);
                let mut extensions: ExtensionsMut = span.extensions_mut();
                extensions.insert(SpanTimingInfo::for_call_path_idx(call_path_idx));
            }
        };
    }

    fn on_enter(&self, _id: &tracing::Id, ctx: Context<S>) {
        let leave_parent = self.clock.end();
        let span = ctx.lookup_current().expect("no span in new_span");

        let mut extensions = span.extensions_mut();
        if let Some(timing_info) = extensions.get_mut::<SpanTimingInfo>() {
            let mut per_thread = timing_info
                .per_thread
                .entry(std::thread::current().id())
                .or_default();
            let start = self.clock.start();
            per_thread.last_enter = start;
            per_thread.last_enter_own = start;
        } else {
            // completely ignore, do not update parent
            return
        }
        // do not lock multiple extensions at once
        std::mem::drop(extensions);

        if let Some(parent) = span.parent() {
            let mut extensions = parent.extensions_mut();
            if let Some(timing_info) = extensions.get_mut::<SpanTimingInfo>() {
                let last_enter_own =
                    timing_info.per_thread[&std::thread::current().id()].last_enter_own;
                let delta = self.clock.delta(last_enter_own, leave_parent);
                timing_info.sum_own += delta;
            }
        }
    }

    fn on_exit(&self, id: &tracing::Id, ctx: Context<'_, S>) {
        let end = self.clock.end();
        let span = ctx.span(id).unwrap();

        let mut extensions = span.extensions_mut();
        let timing_info = extensions.get_mut::<SpanTimingInfo>();
        if timing_info.is_none() {
            return;
        }
        let timing_info = timing_info.unwrap();
        let per_thread = &timing_info.per_thread[&std::thread::current().id()];
        let wall_duration = self.clock.delta(per_thread.last_enter, end);
        timing_info.sum_with_children += wall_duration;
        let own_duration = self.clock.delta(per_thread.last_enter_own, end);
        timing_info.sum_own += own_duration;

        // It is likely that we will be entered by the same thread again,
        // but we do not want to bloat memory if we are constantly entered
        // in different threads.
        timing_info.per_thread.remove(&std::thread::current().id());

        // Make sure that we do not hold two extension locks at once.
        std::mem::drop(extensions);

        if let Some(parent) = span.parent() {
            let mut extensions = parent.extensions_mut();
            let timing_info = extensions
                .get_mut::<SpanTimingInfo>()
                .expect("parent has no SpanTimingInfo");
            let enter_own = self.clock.start();
            timing_info
                .per_thread
                .entry(std::thread::current().id())
                .and_modify(|per_thread| {
                    per_thread.last_enter_own = enter_own;
                });
        }
    }

    fn on_close(&self, id: Id, ctx: Context<S>) {
        let span = ctx.span(&id).expect("no span in close");
        let mut extensions = span.extensions_mut();
        let timing_info = extensions.remove::<SpanTimingInfo>();
        if timing_info.is_none() {
            return;
        }
        let timing_info = timing_info.unwrap();
        let root_extensions_opt = span.from_root().next();
        let mut root_extensions: ExtensionsMut = match root_extensions_opt.as_ref() {
            Some(re) => {
                // Make sure that we do not hold two extension locks at once.
                std::mem::drop(extensions);
                re.extensions_mut()
            }
            None => extensions,
        };

        let pool: &mut CallPathPool = root_extensions
            .get_mut::<CallPathPool>()
            .expect("no pool in root Span");
        let call_path_timing: &mut CallPathTiming = &mut pool[timing_info.call_path_idx];
        call_path_timing.call_count += 1;
        call_path_timing.sum_with_children += timing_info.sum_with_children;
        call_path_timing.sum_own += timing_info.sum_own;

        if span.parent().is_none() {
            let pool = root_extensions
                .remove::<CallPathPool>()
                .expect("no pool in root Span");

            self.processor.process_finished_call(pool);
        }
    }
}

#[cfg(test)]
pub(crate) mod test {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use futures::channel::mpsc::{channel, Receiver, Sender};
    use quanta::{Clock, Mock};
    use tracing::{info, Instrument};
    use tracing_subscriber::fmt;

    use crate::{CallPathPool, CallTreeCollectorBuilder, FinishedCallTreeProcessor};

    #[tracing::instrument]
    pub fn one_ns(mock: &Mock) {
        mock.increment(1);
    }

    #[test]
    fn test_simple() {
        let call_trees = collect_call_trees(|mock| {
            one_ns(&mock);
        });

        assert_eq!(call_trees.len(), 1, "{:#?}", call_trees);

        let first_call = &call_trees[0];
        assert_eq!(first_call.pool.len(), 1, "{:#?}", first_call.pool);
        let first_call_root = first_call.root();
        assert_eq!(
            first_call_root.static_span_meta().name(),
            "one_ns",
            "{:#?}",
            first_call
        );
        assert_eq!(first_call_root.call_count(), 1, "{:#?}", first_call);
        assert_eq!(
            first_call_root.sum_with_children(),
            Duration::from_nanos(1),
            "{:#?}",
            first_call
        );
        assert_eq!(
            first_call_root.sum_without_children(),
            Duration::from_nanos(1),
            "{:#?}",
            first_call
        );
    }

    #[tracing::instrument]
    pub fn compound_call(mock: &Mock) {
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

        assert_eq!(call_trees.len(), 1, "{:#?}", call_trees);

        let first_call = &call_trees[0];
        assert_eq!(first_call.pool.len(), 2, "{:#?}", first_call.pool);

        let first_call_root = first_call.root();
        assert_eq!(
            first_call_root.static_span_meta().name(),
            "compound_call",
            "{:#?}",
            first_call
        );
        assert_eq!(first_call_root.call_count(), 1, "{:#?}", first_call);
        assert_eq!(
            first_call_root.sum_with_children(),
            Duration::from_nanos(1113),
            "{:#?}",
            first_call
        );
        assert_eq!(
            first_call_root.sum_without_children(),
            Duration::from_nanos(1110),
            "{:#?}",
            first_call
        );
        assert_eq!(first_call_root.children().count(), 1, "{:#?}", call_trees);

        let nested_call_idx = *first_call_root.children().next().unwrap();
        let nested_call = &first_call[nested_call_idx];
        assert_eq!(nested_call.static_span_meta().name(), "one_ns");
        assert_eq!(nested_call.call_count(), 3);
        assert_eq!(nested_call.sum_with_children(), Duration::from_nanos(3));
        assert_eq!(nested_call.sum_without_children(), Duration::from_nanos(3));
    }

    #[tracing::instrument(skip(mock, receiver))]
    pub async fn eat_three(mock: Arc<Mock>, mut receiver: Receiver<usize>) {
        use futures::StreamExt;
        for _ in 0..3 {
            let _next = receiver.next().await.unwrap();
            info!("increment 1_000");
            mock.increment(1_000);
        }
    }

    #[tracing::instrument(skip(mock, sender))]
    pub async fn cook_three(mock: Arc<Mock>, mut sender: Sender<usize>) {
        use futures::SinkExt;
        for _ in 0..3 {
            info!("increment 10_000");
            mock.increment(10_000);
            sender.send(0).await.unwrap();
        }
    }

    #[tracing::instrument(skip(mock))]
    pub async fn cooking_party(mock: Arc<Mock>) {
        // Use "no" buffer (which means a buffer of one for each sender)
        // to enforce a deterministic order.
        let (sender, receiver) = channel(0);
        use tracing_futures::WithSubscriber;
        info!("increment 1_000_000");
        mock.increment(1_000_000);

        let handle = tokio::spawn({
            let mock = mock.clone();
            async {
                eat_three(mock, receiver).await;
            }
            .in_current_span()
            .with_current_subscriber()
        });

        info!("increment 10_000_000");
        mock.increment(10_000_000);
        cook_three(mock.clone(), sender).await;

        handle.await.unwrap();
        info!("increment 100_000_000");
        mock.increment(100_000_000);
    }

    #[test]
    fn test_with_futures() {
        let call_tree = collect_call_trees(|mock| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                cooking_party(mock).await;
            });
        });

        println!("{:#?}", call_tree);
    }

    pub fn collect_call_trees(call: impl Fn(Arc<Mock>) -> ()) -> Vec<CallPathPool> {
        use tracing_subscriber::prelude::*;

        let call_trees = FinishedCallTreeStore::default();
        {
            let (clock, mock) = Clock::mock();
            let call_tree_collector = CallTreeCollectorBuilder::default()
                .clock(clock)
                .build_with_collector(call_trees.clone());
            let fmt_layer = fmt::layer()
                .with_thread_ids(true)
                .without_time()
                .with_target(false);
            let subscriber = tracing_subscriber::registry()
                .with(call_tree_collector)
                .with(fmt_layer);
            tracing::subscriber::with_default(subscriber, || {
                call(mock);
            });
        }
        call_trees.to_vec()
    }

    #[derive(Clone, Default)]
    struct FinishedCallTreeStore {
        store: Arc<Mutex<Vec<CallPathPool>>>,
    }

    impl FinishedCallTreeStore {
        pub fn to_vec(self) -> Vec<CallPathPool> {
            let mut arc = self.store;
            // Really not sure why we have some asynchronous use here.
            let store = loop {
                match Arc::try_unwrap(arc) {
                    Ok(store) => break store,
                    Err(a) => arc = a,
                }
            };

            store
                .into_inner()
                .unwrap()
                .into()
        }
    }

    impl FinishedCallTreeProcessor for FinishedCallTreeStore {
        fn process_finished_call(&self, pool: CallPathPool) {
            let mut guard = self.store.lock().expect("getting collect log");
            guard.push(pool);
        }
    }
}
