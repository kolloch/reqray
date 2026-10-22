//! Log "request x-rays" for rust programs instrumented with
//! [tracing](https://github.com/tokio-rs/tracing). This includes aggregated
//! wall/own times as frequently found in flame graphs in a human-friendly text
//! format. Example:
//!
//! ```text
//! 2022-02-06T20:01:57.103747Z  INFO Call summary of request@examples/nested.rs:51
//!
//!                         ## calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
//!                     ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
//!                           0 001 ┊      258.910 ┊      258.890 ┊         0.106 ┊ ┬ request
//!                           0 001 ┊       87.204 ┊       87.190 ┊        19.299 ┊ ├┬ nested
//!                           0 001 ┊        0.036 ┊        0.021 ┊         0.021 ┊ ┊├─ random
//!                           1 000 ┊       75.738 ┊       61.912 ┊        61.912 ┊ ┊╰─ repeated
//!                           0 002 ┊        0.051 ┊        0.027 ┊         0.027 ┊ ├─ repeated
//!                           0 001 ┊        1.644 ┊        1.632 ┊         0.019 ┊ ├┬ nest_deeply
//!                           0 001 ┊        1.619 ┊        1.607 ┊         0.025 ┊ ┊╰┬ nest_deeply
//!                           0 001 ┊        1.593 ┊        1.577 ┊         0.024 ┊ ┊ ╰┬ nest_deeply
//!                           0 001 ┊        1.561 ┊        1.547 ┊         0.022 ┊ ┊  ╰┬ nest_deeply
//!                           0 001 ┊        1.532 ┊        1.520 ┊         0.023 ┊ ┊   ╰┬ nest_deeply
//!                           0 001 ┊        1.504 ┊        1.492 ┊         0.023 ┊ ┊    ╰┬ nest_deeply
//!                           0 001 ┊        1.476 ┊        1.463 ┊         0.025 ┊ ┊     ╰┬ nest_deeply
//!                           0 001 ┊        1.446 ┊        1.433 ┊         0.025 ┊ ┊      ╰┬ nest_deeply
//!                           0 001 ┊        1.415 ┊        1.402 ┊         1.402 ┊ ┊       ╰─ nest_deeply
//!                           0 001 ┊      169.915 ┊      169.905 ┊        17.883 ┊ ╰┬ nested2
//!                           0 001 ┊        0.010 ┊        0.001 ┊         0.001 ┊  ├─ random
//!                           1 000 ┊       88.793 ┊       76.081 ┊        76.081 ┊  ├─ repeated
//!                           0 001 ┊       70.386 ┊       70.377 ┊        19.332 ┊  ╰┬ nested
//!                           0 001 ┊        0.011 ┊        0.001 ┊         0.001 ┊   ├─ random
//!                           1 000 ┊       58.468 ┊       45.280 ┊        45.280 ┊   ╰─ repeated
//! ```
//!
//! * **calls**: The total number of spans closed at this call path.
//! * **∑ alive ms**: The total time spans at this call path were alive i.e.
//!   sum of times between new and close events.
//! * **∑ busy ms**: The total time spans at this call path were entered i.e.
//!   sum of times between enter and exit events. A span entered on several
//!   threads at once is busy on each of them; a span entered again on a
//!   thread where it is entered already (e.g. a future instrumented with the
//!   current span) counts once.
//! * **∑ own busy ms**: The total time spans at this call path were the
//!   innermost entered span of a thread: busy time minus the time another
//!   span was entered on top of them. On one thread, the own times of a
//!   call tree add up to the busy time of its root, unless a span of
//!   another tree was entered on top of one of its spans.
//!
//! Spans beyond the maximum call depth are not recorded: they do not count
//! as entered, so their time goes to the recorded span entered last on the
//! thread, as if their code were inlined into it.
//!
//! Under the hood, `reqray` provides a [`CallTreeCollector`] tracing `Layer`
//! which, unsurprisingly, collects call trees. Once the root span (e.g. the top
//! most span = the top most instrumented call) has been closed, the finished
//! call tree is handed over to a [`FinishedCallTreeProcessor`].
//! [`LoggingCallTreeCollector`] implements [`FinishedCallTreeProcessor`] and
//! logs each call tree in human-friendly way as shown above.
//!
//! Let's assume that you already have an explicit setup for `tracing` like
//! this, then you simply need to add the highlighted line:
//!
//! ```
//!     use reqray::CallTreeCollector;
//!     use tracing_subscriber::{EnvFilter, util::SubscriberInitExt, fmt, prelude::*};
//!
//!     let fmt_layer = fmt::layer()
//!         .with_target(false);
//!     let filter_layer = EnvFilter::try_from_default_env()
//!         .or_else(|_| EnvFilter::try_new("info"))
//!         .unwrap();
//!
//!     tracing_subscriber::registry()
//!     // -----------------------------------------------
//!         .with(CallTreeCollector::default())
//!     // -----------------------------------------------
//!         .with(filter_layer)
//!         .with(fmt_layer)
//!         .init();
//! ```
//!
//! Instead of `CallTreeCollector::default()` you can chose a more explicit
//! config using [`CallTreeCollectorBuilder`] and
//! [`LoggingCallTreeCollectorBuilder`].
//!
//! ```
//! use reqray::{CallTreeCollectorBuilder, display::LoggingCallTreeCollectorBuilder};
//! use tracing_subscriber::{EnvFilter, util::SubscriberInitExt, fmt, prelude::*};
//!
//! # let fmt_layer = fmt::layer().with_target(false);
//! # let filter_layer = EnvFilter::try_from_default_env()
//! #   .or_else(|_| EnvFilter::try_new("info"))
//! #   .unwrap();
//! // ...
//! let call_tree_collector = CallTreeCollectorBuilder::default()
//!     .max_call_depth(10)
//!     .build_with_collector(
//!         LoggingCallTreeCollectorBuilder::default()
//!             .left_margin(20)
//!             .build(),
//!     );
//!
//! tracing_subscriber::registry()
//!     .with(call_tree_collector)
//!     // ...
//! #    .with(filter_layer)
//! #    .with(fmt_layer)
//! #    .init();
//! ```

pub mod display;
mod internal;

use std::fmt;

use display::{LoggingCallTreeCollector, LoggingCallTreeCollectorBuilder};
use quanta::Clock;

// These are internal and republished here to force code in the
// display model to use the public interface.
pub use internal::{CallPathPool, CallPathPoolId, CallPathTiming};

/// A [`tracing_subscriber::Layer`] which collects call trees and hands
/// finished trees to a [`FinishedCallTreeProcessor`].
///
/// Use [`CallTreeCollector::default()`] if you want to log all call trees with
/// the standard configuration.
///
/// Use [`CallTreeCollectorBuilder`] together with e.g.
/// [`LoggingCallTreeCollectorBuilder`] to customize your setup.
pub struct CallTreeCollector<H: FinishedCallTreeProcessor + 'static> {
    /// The clock to use for determing call timings.
    clock: Clock,
    /// Ignore calls beyond this depth.
    max_call_depth: usize,
    processor: H,
    /// Every thread's stack of entered spans.
    entered: internal::EnteredStacks,
}

impl<H: FinishedCallTreeProcessor + 'static> fmt::Debug for CallTreeCollector<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallTreeCollector")
            .field("clock", &self.clock)
            .field("max_call_depth", &self.max_call_depth)
            .finish_non_exhaustive()
    }
}

impl Default for CallTreeCollector<LoggingCallTreeCollector> {
    fn default() -> Self {
        CallTreeCollectorBuilder::default()
            .build_with_collector(LoggingCallTreeCollectorBuilder::default().build())
    }
}

/// A [`FinishedCallTreeProcessor`] uses the aggregated call tree for
/// something useful.
///
/// Expected use cases:
///
/// * Log the call tree
/// * Generate metrics from the call tree
/// * Do anomaly detection on the call tree
/// * Send the call tree elsewhere for further aggregation
///
/// It is called from within the `tracing` subscriber, when the root span
/// of the tree closes, on the thread that closed it. It may emit `tracing`
/// events, as [`LoggingCallTreeCollector`] does, but it must not create
/// spans: a span closed while another span is closing is never freed by
/// `tracing_subscriber`'s registry, and the tree it belongs to would never
/// be reported.
pub trait FinishedCallTreeProcessor: Send + Sync {
    fn process_finished_call(&self, pool: CallPathPool);
}

/// Configure & Build [`CallTreeCollector`]s.
///
/// Example:
///
/// ```
/// use reqray::{CallTreeCollectorBuilder, display::LoggingCallTreeCollectorBuilder};
///
/// let collector =
///     CallTreeCollectorBuilder::default()
///         .max_call_depth(42)
///         .build_with_collector(
///              LoggingCallTreeCollectorBuilder::default()
///                  .left_margin(20)
///                  .build()
///         );
/// ```
#[derive(Debug)]
pub struct CallTreeCollectorBuilder {
    clock: Option<Clock>,
    max_call_depth: usize,
}

impl Default for CallTreeCollectorBuilder {
    fn default() -> Self {
        CallTreeCollectorBuilder {
            clock: None,
            max_call_depth: 10,
        }
    }
}

impl CallTreeCollectorBuilder {
    /// The clock to use for measure execution time.
    ///
    /// The default is to use a real clock, but you can pass
    /// in a mock clock for testing.
    #[must_use]
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The maximum call depth of the call tree to record -- must be
    /// at least `2`, which records the root span and its direct children.
    ///
    /// Call paths below this depth are not recorded -- so their execution
    /// is recorded as if they were inlined into the deepest recorded span.
    #[must_use]
    pub fn max_call_depth(mut self, max_call_depth: usize) -> Self {
        self.max_call_depth = max_call_depth;
        self
    }

    /// Build the [`CallTreeCollector`] handing over the finished call trees
    /// to `processor`.
    pub fn build_with_collector<H>(self, processor: H) -> CallTreeCollector<H>
    where
        H: FinishedCallTreeProcessor + 'static,
    {
        CallTreeCollector {
            clock: self.clock.unwrap_or_default(),
            max_call_depth: core::cmp::max(2, self.max_call_depth),
            processor,
            entered: internal::EnteredStacks::new(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Default)]
    struct NoopProcessor;

    impl FinishedCallTreeProcessor for NoopProcessor {
        fn process_finished_call(&self, _pool: CallPathPool) {}
    }

    #[test]
    fn collector_enforces_minimum_depth() {
        let collector = CallTreeCollectorBuilder::default()
            .max_call_depth(0)
            .build_with_collector(NoopProcessor);

        assert_eq!(collector.max_call_depth, 2);
    }

    /// A layer must be usable from every thread of a subscriber.
    #[test]
    fn collector_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&CallTreeCollector::default());
    }
}
