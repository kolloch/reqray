//! Log "request x-rays" for rust programs instrumented with [tracing](https://github.com/tokio-rs/tracing). This
//! includes aggregated wall/own times as frequently found in flame graphs in a human-friendly text format.
//!
//! Let's assume that you already have an explicit setup for `tracing` like this, then you simply
//! need to add the highlighted line:
//!
//! ```rust
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
//! Instead of `CallTreeCollector::default()` you can chose a more explicit config:
//!
//! ```rust
//!     // ...
//!     let call_tree_collector = CallTreeCollectorBuilder::default()
//!         .max_call_depth(10)
//!         .build_with_collector(
//!             LoggingCallTreeCollectorBuilder::default()
//!                 .left_margin(20)
//!                 .build(),
//!         );
//!
//!     tracing_subscriber::registry()
//!         .with(call_tree_collector)
//!         // ...
//! ```

pub mod display;
mod internal;

use display::{LoggingCallTreeCollector, LoggingCallTreeCollectorBuilder};
use quanta::Clock;

// These are internal and republished here to force code in the
// display model to use the public interface.
pub use internal::{CallPathPool, CallPathPoolId, CallPathTiming};

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

/// A [tracing::Subscriber] which collects call trees
/// and hands finished trees to a [FinishedCallTreeProcessor].
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

/// Configure & Build [CallTreeCollector]s.
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
