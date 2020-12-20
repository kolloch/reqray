pub mod display;
mod internal;

use display::{LoggingCallTreeCollector, LoggingCallTreeCollectorBuilder};
use quanta::Clock;

// These are internal and republished here to force code in the
// display model to use the public interface.
pub use internal::{CallPathPool, CallPathPoolId, CallPathTiming};

/// A [CallTreeProcessor] can act on the finished call tree.
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

/// A [tracing_subscriber::Subscriber] which collects call trees
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
