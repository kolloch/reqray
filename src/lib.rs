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
//! * **calls**: The total number of spans created at this call path.
//! * **∑ alive ms**: The total time spans at this call path were alive i.e. sum of times between new and close events.
//! * **∑ busy ms**: The total time spans at this call path were entered i.e. sum of times between enter and leave events.
//! * **∑ own busy ms**: The total time spans at this call path were entered without any children entered.
//!
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
            clock: self.clock.unwrap_or_else(Clock::new),
            max_call_depth: core::cmp::max(2, self.max_call_depth),
            processor,
        }
    }
}
