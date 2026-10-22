//! Logging finished call trees as text tables.

use core::fmt;

use crate::{CallPathPool, CallPathTiming, FinishedCallTreeProcessor};

/// A [`FinishedCallTreeProcessor`] that logs every finished call tree as a
/// `tracing` event at level `INFO`, as a table with one line per call path.
#[derive(Debug)]
pub struct LoggingCallTreeCollector {
    max_call_depth: usize,
    left_margin: usize,
}

/// Configure & build [`LoggingCallTreeCollector`]s.
#[derive(Debug)]
pub struct LoggingCallTreeCollectorBuilder {
    max_call_depth: usize,
    left_margin: usize,
}

impl LoggingCallTreeCollectorBuilder {
    /// The number of levels of the call tree to log, the root included:
    /// call paths deeper than that are left out of the table. Their times
    /// are still part of the sums of their ancestors.
    ///
    /// The default is `10`. To record fewer levels in the first place, see
    /// [`crate::CallTreeCollectorBuilder::max_call_depth`].
    #[must_use]
    pub fn max_call_depth(mut self, max_call_depth: usize) -> Self {
        self.max_call_depth = max_call_depth;
        self
    }

    /// The number of spaces before every line of the table, so that it
    /// stands out from the log lines around it. The default is `20`.
    #[must_use]
    pub fn left_margin(mut self, left_margin: usize) -> Self {
        self.left_margin = left_margin;
        self
    }

    #[must_use]
    pub fn build(self) -> LoggingCallTreeCollector {
        LoggingCallTreeCollector {
            max_call_depth: self.max_call_depth.max(1),
            left_margin: self.left_margin,
        }
    }
}

impl Default for LoggingCallTreeCollectorBuilder {
    fn default() -> Self {
        LoggingCallTreeCollectorBuilder {
            max_call_depth: 10,
            left_margin: 20,
        }
    }
}

impl FinishedCallTreeProcessor for LoggingCallTreeCollector {
    fn process_finished_call(&self, pool: CallPathPool) {
        let root = pool.root();
        tracing::info!(
            "Call summary of {}@{}:{}\n\n{}",
            root.static_span_meta().name(),
            root.static_span_meta().file().unwrap_or("unknown"),
            root.static_span_meta().line().unwrap_or(0),
            DisplayableCallPathTiming {
                max_call_depth: self.max_call_depth,
                left_margin: self.left_margin,
                pool: &pool,
            }
        );
    }
}

/// The table of a call tree, as [`LoggingCallTreeCollector`] logs it.
#[derive(Debug)]
struct DisplayableCallPathTiming<'a> {
    max_call_depth: usize,
    left_margin: usize,
    pool: &'a CallPathPool,
}

impl fmt::Display for DisplayableCallPathTiming<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:indent$}    # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree",
            "",
            indent = self.left_margin
        )?;
        writeln!(
            f,
            "{:indent$}────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────",
            "",
            indent = self.left_margin
        )?;
        let mut last = Vec::new();
        last.push(true);
        self.fmt_node(&mut last, self.pool.root(), f)
    }
}

impl DisplayableCallPathTiming<'_> {
    /// One line for `node` and then its children, which it finds in the
    /// pool. `last` says, for every level above, whether the ancestor at
    /// that level is the last child of its parent (no line to continue).
    fn fmt_node(
        &self,
        last: &mut Vec<bool>,
        node: &CallPathTiming,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        write!(
            f,
            "{:indent$}{: >7} {:0>3} ┊ {: >8}.{:0>3} ┊ {: >8}.{:0>3} ┊  {: >8}.{:0>3} ┊ ",
            "",
            node.call_count() / 1000,
            node.call_count() % 1000,
            node.span_alive().as_micros() / 1000,
            node.span_alive().as_micros() % 1000,
            node.sum_with_children().as_micros() / 1000,
            node.sum_with_children().as_micros() % 1000,
            node.sum_without_children().as_micros() / 1000,
            node.sum_without_children().as_micros() % 1000,
            indent = self.left_margin
        )?;

        let mut children = node
            .children()
            .copied()
            .filter(|child| self.pool[*child].depth() < self.max_call_depth)
            .collect::<Vec<_>>();
        children.sort_unstable();

        let child_connector = if children.is_empty() { "─" } else { "┬" };
        if last.len() == 1 {
            writeln!(f, "{} {}", child_connector, node.static_span_meta().name())?;
        } else {
            for is_last in last.iter().skip(1).take(last.len() - 2) {
                f.write_str(if *is_last { " " } else { "┊" })?;
            }
            let connect_me = if last.last().copied().unwrap_or(true) {
                "╰"
            } else {
                "├"
            };
            f.write_str(connect_me)?;
            f.write_str(child_connector)?;
            writeln!(f, " {}", node.static_span_meta().name())?;
        }

        for (idx, child_idx) in children.iter().enumerate() {
            let child = &self.pool[*child_idx];
            last.push(idx + 1 == children.len());
            self.fmt_node(last, child, f)?;
            last.pop();
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use quanta::{Clock, Mock};
    use tracing::info_span;
    use tracing_subscriber::fmt::MakeWriter;

    use crate::{
        CallTreeCollectorBuilder,
        internal::test::{
            block_on_single_thread, collect_call_trees, compound_call, cooking_party, one_ns,
        },
    };

    use super::LoggingCallTreeCollectorBuilder;

    #[test]
    fn display_one_ns() {
        let str = display_call_trees(10, |mock| one_ns(&mock));
        assert_eq!(
            &str,
            indoc::indoc! {r"
                    # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
                ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                      0 001 ┊        0.000 ┊        0.000 ┊         0.000 ┊ ─ one_ns

            "},
            "got:\n{str}"
        );
    }

    #[test]
    fn display_compound_call() {
        let str = display_call_trees(10, |mock| compound_call(&mock));
        assert_eq!(
            &str,
            indoc::indoc! {r"
                # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
            ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                  0 001 ┊        0.001 ┊        0.001 ┊         0.001 ┊ ┬ compound_call
                  0 003 ┊        0.000 ┊        0.000 ┊         0.000 ┊ ╰─ one_ns

            "},
            "got:\n{str}"
        );
    }

    #[tracing::instrument]
    fn nest_deeply(mock: &Mock, nest: usize) {
        if nest == 0 {
            return;
        }

        mock.increment(1_000);
        nest_deeply(mock, nest - 1);
        mock.increment(1_000_000);
    }

    /// The collector records ten levels: the two deepest calls are inlined
    /// into the tenth.
    #[test]
    fn display_nest_deeply() {
        let str = display_call_trees(10, |mock| nest_deeply(&mock, 11));
        assert_eq!(
            &str,
            indoc::indoc! {r"
                # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
            ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                  0 001 ┊       11.011 ┊       11.011 ┊         1.001 ┊ ┬ nest_deeply
                  0 001 ┊       10.010 ┊       10.010 ┊         1.001 ┊ ╰┬ nest_deeply
                  0 001 ┊        9.009 ┊        9.009 ┊         1.001 ┊  ╰┬ nest_deeply
                  0 001 ┊        8.008 ┊        8.008 ┊         1.001 ┊   ╰┬ nest_deeply
                  0 001 ┊        7.007 ┊        7.007 ┊         1.001 ┊    ╰┬ nest_deeply
                  0 001 ┊        6.006 ┊        6.006 ┊         1.001 ┊     ╰┬ nest_deeply
                  0 001 ┊        5.005 ┊        5.005 ┊         1.001 ┊      ╰┬ nest_deeply
                  0 001 ┊        4.004 ┊        4.004 ┊         1.001 ┊       ╰┬ nest_deeply
                  0 001 ┊        3.003 ┊        3.003 ┊         1.001 ┊        ╰┬ nest_deeply
                  0 001 ┊        2.002 ┊        2.002 ┊         2.002 ┊         ╰─ nest_deeply

            "},
            "got:\n{str}"
        );
    }

    /// The logger's own depth limit cuts the table, not the sums.
    #[test]
    fn display_max_call_depth_cuts_the_table() {
        let str = display_call_trees(3, |mock| nest_deeply(&mock, 11));
        assert_eq!(
            &str,
            indoc::indoc! {r"
                # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
            ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                  0 001 ┊       11.011 ┊       11.011 ┊         1.001 ┊ ┬ nest_deeply
                  0 001 ┊       10.010 ┊       10.010 ┊         1.001 ┊ ╰┬ nest_deeply
                  0 001 ┊        9.009 ┊        9.009 ┊         1.001 ┊  ╰─ nest_deeply

            "},
            "got:\n{str}"
        );
    }

    /// Siblings with children: the lines continue past a child that is not
    /// the last.
    #[test]
    fn display_tree_lines() {
        let str = display_call_trees(10, |mock| {
            let root = info_span!("root");
            let _root = root.enter();
            {
                let first = info_span!("first");
                let _first = first.enter();
                let leaf = info_span!("leaf");
                let _leaf = leaf.enter();
                mock.increment(1_000_000);
            }
            {
                let second = info_span!("second");
                let _second = second.enter();
                let leaf = info_span!("leaf");
                let _leaf = leaf.enter();
                mock.increment(2_000_000);
            }
        });
        assert_eq!(
            &str,
            indoc::indoc! {r"
                # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
            ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                  0 001 ┊        3.000 ┊        3.000 ┊         0.000 ┊ ┬ root
                  0 001 ┊        1.000 ┊        1.000 ┊         0.000 ┊ ├┬ first
                  0 001 ┊        1.000 ┊        1.000 ┊         1.000 ┊ ┊╰─ leaf
                  0 001 ┊        2.000 ┊        2.000 ┊         0.000 ┊ ╰┬ second
                  0 001 ┊        2.000 ┊        2.000 ┊         2.000 ┊  ╰─ leaf

            "},
            "got:\n{str}"
        );
    }

    #[test]
    fn display_with_futures() {
        let str = display_call_trees(10, |mock| {
            block_on_single_thread(cooking_party(mock));
        });
        assert_eq!(
            &str,
            indoc::indoc! {r"
                # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
            ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                  0 001 ┊      101.033 ┊      101.033 ┊       101.000 ┊ ┬ cooking_party
                  0 001 ┊        0.033 ┊        0.003 ┊         0.003 ┊ ├─ eat_three
                  0 001 ┊        0.033 ┊        0.030 ┊         0.030 ┊ ╰─ cook_three

            "},
            "got:\n{str}"
        );
    }

    /// Everything a log line of `tracing_subscriber::fmt` writes.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Log {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Log {
        type Writer = Log;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The logging collector logs the table through the other layers of the
    /// subscriber, from within the close of the root span.
    #[test]
    fn logging_collector_logs_the_table() {
        use tracing_subscriber::prelude::*;

        let log = Log::default();
        let (clock, mock) = Clock::mock();
        let collector = CallTreeCollectorBuilder::default()
            .clock(clock)
            .build_with_collector(
                LoggingCallTreeCollectorBuilder::default()
                    .left_margin(2)
                    .build(),
            );
        let subscriber = tracing_subscriber::registry().with(collector).with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(log.clone()),
        );
        tracing::subscriber::with_default(subscriber, || {
            compound_call(&mock);
        });

        let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        let expected = indoc::indoc! {r"
             INFO reqray::display: Call summary of compound_call@src/internal.rs:

                   # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
               ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                     0 001 ┊        0.001 ┊        0.001 ┊         0.001 ┊ ┬ compound_call
                     0 003 ┊        0.000 ┊        0.000 ┊         0.000 ┊ ╰─ one_ns

        "};
        // The line number of `compound_call` changes with the tests.
        let marker = "src/internal.rs:";
        let (prefix, rest) = expected.split_once(marker).unwrap();
        let (header, table) = rest.split_once('\n').unwrap();
        assert_eq!(header, "");
        let prefix = format!("{prefix}{marker}");
        let start = logged
            .find(&prefix)
            .unwrap_or_else(|| panic!("got:\n{logged}"));
        let after_prefix = &logged[start + prefix.len()..];
        let (line, logged_table) = after_prefix.split_once('\n').unwrap();
        assert!(
            line.parse::<u32>().is_ok(),
            "line number, got {line:?} in:\n{logged}"
        );
        assert_eq!(logged_table, table, "got:\n{logged}");
    }

    fn display_call_trees(max_call_depth: usize, call: impl FnOnce(Arc<Mock>)) -> String {
        use std::fmt::Write;

        let call_trees = collect_call_trees(call);

        let mut out = String::new();
        for call_tree in call_trees {
            writeln!(
                &mut out,
                "{}",
                super::DisplayableCallPathTiming {
                    max_call_depth,
                    left_margin: 0,
                    pool: &call_tree,
                }
            )
            .unwrap();
        }
        out
    }
}
