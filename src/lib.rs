//! Log "request x-rays" for rust programs instrumented with
//! [tracing](https://github.com/tokio-rs/tracing). This includes aggregated
//! wall/own times as frequently found in flame graphs in a human-friendly text
//! format. Example:
//!
//! ```text
//! Dec 20 18:48:32.405  INFO Call summary of request@examples/nested.rs:47
//!
//!                         # calls │    ∑ wall ms │     ∑ own ms │ span tree
//!                     ────────────┼──────────────┼──────────────┼───────────────────────
//!                           0 001 ┊      377.886 ┊        0.260 ┊ ┬ request
//!                           0 001 ┊      120.704 ┊       48.896 ┊ ├┬ nested
//!                           0 001 ┊        0.008 ┊        0.008 ┊ ┊├─ random
//!                           1 000 ┊       64.347 ┊       64.347 ┊ ┊╰─ repeated
//!                           0 002 ┊        0.118 ┊        0.118 ┊ ├─ repeated
//!                           0 001 ┊        3.818 ┊        0.049 ┊ ├┬ nest_deeply
//!                           0 001 ┊        3.762 ┊        0.053 ┊ ┊╰┬ nest_deeply
//!                           0 001 ┊        3.702 ┊        0.057 ┊ ┊ ╰┬ nest_deeply
//!                           0 001 ┊        3.637 ┊        0.056 ┊ ┊  ╰┬ nest_deeply
//!                           0 001 ┊        3.574 ┊        0.058 ┊ ┊   ╰┬ nest_deeply
//!                           0 001 ┊        3.503 ┊        0.061 ┊ ┊    ╰┬ nest_deeply
//!                           0 001 ┊        3.435 ┊        0.063 ┊ ┊     ╰┬ nest_deeply
//!                           0 001 ┊        3.365 ┊        0.066 ┊ ┊      ╰┬ nest_deeply
//!                           0 001 ┊        3.292 ┊        3.292 ┊ ┊       ╰─ nest_deeply
//!                           0 001 ┊      252.949 ┊       49.258 ┊ ╰┬ nested2
//!                           0 001 ┊        0.006 ┊        0.006 ┊  ├─ random
//!                           1 000 ┊       63.343 ┊       63.343 ┊  ├─ repeated
//!                           0 001 ┊      132.841 ┊       54.091 ┊  ╰┬ nested
//!                           0 001 ┊        0.007 ┊        0.007 ┊   ├─ random
//!                           1 000 ┊       70.875 ┊       70.875 ┊   ╰─ repeated
//!
//! ```
//!
//! Under the hood, `reqray` provides a [CallTreeCollector] tracing `Layer`
//! which, unsurprisingly, collects call trees. Once the root span (e.g. the top
//! most span = the top most instrumented call) has been closed, the finished
//! call tree is handed over to a [FinishedCallTreeProcessor].
//! [LoggingCallTreeCollector] implements [FinishedCallTreeProcessor] and logs
//! each call tree in human-friendly way as shown above.
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
//! config using [CallTreeCollectorBuilder] and
//! [LoggingCallTreeCollectorBuilder].
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

use display::{LoggingCallTreeCollector, LoggingCallTreeCollectorBuilder};
use quanta::Clock;

// These are internal and republished here to force code in the
// display model to use the public interface.
pub use internal::{CallPathPool, CallPathPoolId, CallPathTiming};

/// A [tracing::Subscriber] which collects call trees and hands finished trees
/// to a [FinishedCallTreeProcessor].
///
/// Use [CallTreeCollector::default()] if you want to log all call trees with
/// the standard configuration.
///
/// Use [CallTreeCollectorBuilder] together with e.g.
/// [LoggingCallTreeCollectorBuilder] to customize your setup.
pub struct CallTreeCollector<H: FinishedCallTreeProcessor + 'static> {
    /// The clock to use for determing call timings.
    clock: Clock,
    /// Ignore calls beyond this depth.
    max_call_depth: usize,
    processor: H,
}

impl Default for CallTreeCollector<LoggingCallTreeCollector> {
    fn default() -> Self {
        CallTreeCollectorBuilder::default()
            .build_with_collector(LoggingCallTreeCollectorBuilder::default().build())
    }
}

/// A [FinishedCallTreeProcessor] uses the aggregated call tree for
/// something useful.
///
/// Expected use cases:
///
/// * Log the call tree
/// * Generate metrics from the call tree
/// * Do anamoly detection on the call tree
/// * Send the call tree elswhere for further aggregation
pub trait FinishedCallTreeProcessor {
    fn process_finished_call(&self, pool: CallPathPool);
}

/// Configure & Build [CallTreeCollector]s.
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
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The maximum call depth of the call tree to record -- must be
    /// at least `2`.
    ///
    /// Call paths below this depth are capped -- so their execution
    /// is recorded as if they were inlined.
    pub fn max_call_depth(mut self, max_call_depth: usize) -> Self {
        self.max_call_depth = max_call_depth;
        self
    }

    /// Build the [CallTreeCollector] handing over the finished call trees
    /// to `collector`.
    pub fn build_with_collector<H>(self, processor: H) -> CallTreeCollector<H>
    where
        H: FinishedCallTreeProcessor + 'static,
    {
        CallTreeCollector {
            clock: self.clock.unwrap_or_else(|| Clock::new()),
            max_call_depth: core::cmp::max(2, self.max_call_depth),
            processor,
        }
    }
}
