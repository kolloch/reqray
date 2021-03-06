use reqray::{display::LoggingCallTreeCollectorBuilder, CallTreeCollectorBuilder};
use tracing_subscriber::{fmt, prelude::*, util::SubscriberInitExt, EnvFilter};

fn main() {
    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    let call_tree_collector = CallTreeCollectorBuilder::default()
        .max_call_depth(10)
        .build_with_collector(
            LoggingCallTreeCollectorBuilder::default()
                .left_margin(20)
                .build(),
        );

    tracing_subscriber::registry()
        .with(call_tree_collector)
        .with(filter_layer)
        .with(fmt_layer)
        .init();

    use tracing::{info, instrument};

    #[instrument]
    fn repeated(repetition: i32) {
        info!("repetition: {}", repetition);
    }

    #[instrument]
    fn random() {}

    #[instrument]
    fn nested() {
        random();
        for i in 1..=1000 {
            repeated(i);
        }
    }

    #[instrument]
    fn nested2() {
        random();
        for i in 1..=1000 {
            repeated(i);
        }
        nested();
    }

    #[instrument]
    fn request() {
        nested();
        repeated(-1);
        repeated(-2);
        nest_deeply(100);
        // Even though the name is the same, this is a different span.
        // let name_clash_span = info_span!("nested");
        // let _enter = name_clash_span.enter();
        nested2();
    }

    #[instrument]
    fn nest_deeply(nest: usize) {
        if nest == 0 {
            return;
        }

        nest_deeply(nest - 1);
    }

    request();
}
