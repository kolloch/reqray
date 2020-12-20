use core::fmt;

use crate::{CallPathPool, CallPathTiming, FinishedCallTreeProcessor};

pub struct LoggingCallTreeCollector {
    max_call_depth: usize,
    left_margin: usize,
}

pub struct LoggingCallTreeCollectorBuilder {
    max_call_depth: usize,
    left_margin: usize,
}

impl LoggingCallTreeCollectorBuilder {
    pub fn max_call_depth(mut self, max_call_depth: usize) -> Self {
        self.max_call_depth = max_call_depth;
        self
    }

    pub fn left_margin(mut self, left_margin: usize) -> Self {
        self.left_margin = left_margin;
        self
    }

    pub fn build(self) -> LoggingCallTreeCollector {
        LoggingCallTreeCollector {
            max_call_depth: self.max_call_depth,
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
                root
            }
        )
    }
}

#[derive(Debug)]
struct DisplayableCallPathTiming<'a> {
    max_call_depth: usize,
    left_margin: usize,
    pool: &'a CallPathPool,
    root: &'a CallPathTiming,
}

impl<'a> fmt::Display for DisplayableCallPathTiming<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:indent$}    # calls │    ∑ wall ms │     ∑ own ms │ span tree",
            "",
            indent = self.left_margin
        )?;
        writeln!(
            f,
            "{:indent$}────────────┼──────────────┼──────────────┼───────────────────────",
            "",
            indent = self.left_margin
        )?;
        let mut last = Vec::with_capacity(self.max_call_depth);
        last.push(true);
        self.fmt(&mut last, self.root, f)
    }
}

impl DisplayableCallPathTiming<'_> {
    fn fmt(
        &self,
        // this is wasteful
        last: &mut Vec<bool>,
        node: &CallPathTiming,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        write!(
            f,
            "{:indent$}{: >7} {:0>3} ┊ {: >8}.{:0>3} ┊ {: >8}.{:0>3} ┊ ",
            "",
            node.call_count() / 1000,
            node.call_count() % 1000,
            node.sum_with_children().as_micros() / 1000,
            node.sum_with_children().as_micros() % 1000,
            node.sum_without_children().as_micros() / 1000,
            node.sum_without_children().as_micros() % 1000,
            indent = self.left_margin
        )?;

        let child_connector = if node.children().next().is_none() {
            "─"
        } else {
            "┬"
        };
        match last.len() {
            1 => writeln!(f, "{} {}", child_connector, node.static_span_meta().name())?,
            _ => {
                if last.len() > 2 {
                    for is_last in last.iter().skip(1).take(last.len() - 2) {
                        f.write_str(if *is_last { " " } else { "┊" })?;
                    }
                }

                let connect_me = if *last.iter().last().unwrap() {
                    "╰"
                } else {
                    "├"
                };
                f.write_str(connect_me)?;
                f.write_str(child_connector)?;

                writeln!(f, " {}", node.static_span_meta().name())?;
            }
        };

        let mut children = node.children().copied().collect::<Vec<_>>();
        if children.len() > 0 {
            children.sort();
            let last_dx = children.len() - 1;
            for (idx, child_idx) in children.iter().enumerate() {
                let child = &self.pool[*child_idx];
                last.push(idx == last_dx);
                self.fmt(last, child, f)?;
                last.pop();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use quanta::Mock;

    use crate::internal::test::{collect_call_trees, compound_call, cooking_party, one_ns};

    #[test]
    fn display_one_ns() {
        let str = display_call_trees(|mock| one_ns(&mock));
        assert_eq!(
            &str,
            indoc::indoc! {r#"
                # calls │    ∑ wall ms │     ∑ own ms │ span tree
            ────────────┼──────────────┼──────────────┼───────────────────────
                  0 001 ┊        0.000 ┊        0.000 ┊ ─ one_ns

            "#},
            "got:\n{}",
            str
        );
    }

    #[test]
    fn display_compound_call() {
        let str = display_call_trees(|mock| compound_call(&mock));
        assert_eq!(
            &str,
            indoc::indoc! {r#"
                # calls │    ∑ wall ms │     ∑ own ms │ span tree
            ────────────┼──────────────┼──────────────┼───────────────────────
                  0 001 ┊        0.001 ┊        0.001 ┊ ┬ compound_call
                  0 003 ┊        0.000 ┊        0.000 ┊ ╰─ one_ns

            "#},
            "got:\n{}",
            str
        );
    }

    #[test]
    fn display_with_futures() {
        let str = display_call_trees(|mock| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                cooking_party(mock).await;
            });
        });

        assert_eq!(
            &str,
            indoc::indoc! {r#"
                # calls │    ∑ wall ms │     ∑ own ms │ span tree
            ────────────┼──────────────┼──────────────┼───────────────────────
                  0 001 ┊        0.000 ┊        0.000 ┊ ─ eat_three

                # calls │    ∑ wall ms │     ∑ own ms │ span tree
            ────────────┼──────────────┼──────────────┼───────────────────────
                  0 001 ┊        0.333 ┊        0.300 ┊ ┬ cooking_party
                  0 001 ┊        0.033 ┊        0.033 ┊ ╰─ cook_three

            "#},
            "got:\n{}",
            str
        );
    }

    pub fn display_call_trees(call: impl Fn(Arc<Mock>) -> ()) -> String {
        use std::fmt::Write;

        let call_trees = collect_call_trees(call);

        let mut out = String::new();
        for call_tree in call_trees {
            writeln!(
                &mut out,
                "{}",
                super::DisplayableCallPathTiming {
                    max_call_depth: 10,
                    left_margin: 0,
                    pool: &call_tree,
                    root: call_tree.root()
                }
            )
            .unwrap();
        }
        out
    }
}
