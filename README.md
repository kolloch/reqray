# Reqray

[![Build Status](https://travis-ci.com/kolloch/reqray.svg)](https://travis-ci.com/kolloch/reqray)
[![Crate](https://img.shields.io/crates/v/reqray.svg)](https://crates.io/crates/reqray)
[![Docs](https://docs.rs/reqray/badge.svg)](https://docs.rs/reqray)

Log "request x-rays" for rust programs instrumented with [tracing](https://github.com/tokio-rs/tracing). This
includes aggregated wall/own times as frequently found in flame graphs in a human-friendly text format.

To deploy it, you don't need some complicated services, just some local code added to your instrumented program.

This makes answers to these question often trivial to answer:

* What part of the request takes the most time?
* How many DB requests am I performing? Does the DB request aggregation work?
* How far did the execution get before the error that aborted the request?

It looks like this:

```
Dec 20 18:48:32.405  INFO Call summary of request@examples/nested.rs:47

                        # calls │    ∑ wall ms │     ∑ own ms │ span tree
                    ────────────┼──────────────┼──────────────┼───────────────────────
                          0 001 ┊      377.886 ┊        0.260 ┊ ┬ request
                          0 001 ┊      120.704 ┊       48.896 ┊ ├┬ nested
                          0 001 ┊        0.008 ┊        0.008 ┊ ┊├─ random
                          1 000 ┊       64.347 ┊       64.347 ┊ ┊╰─ repeated
                          0 002 ┊        0.118 ┊        0.118 ┊ ├─ repeated
                          0 001 ┊        3.818 ┊        0.049 ┊ ├┬ nest_deeply
                          0 001 ┊        3.762 ┊        0.053 ┊ ┊╰┬ nest_deeply
                          0 001 ┊        3.702 ┊        0.057 ┊ ┊ ╰┬ nest_deeply
                          0 001 ┊        3.637 ┊        0.056 ┊ ┊  ╰┬ nest_deeply
                          0 001 ┊        3.574 ┊        0.058 ┊ ┊   ╰┬ nest_deeply
                          0 001 ┊        3.503 ┊        0.061 ┊ ┊    ╰┬ nest_deeply
                          0 001 ┊        3.435 ┊        0.063 ┊ ┊     ╰┬ nest_deeply
                          0 001 ┊        3.365 ┊        0.066 ┊ ┊      ╰┬ nest_deeply
                          0 001 ┊        3.292 ┊        3.292 ┊ ┊       ╰─ nest_deeply
                          0 001 ┊      252.949 ┊       49.258 ┊ ╰┬ nested2
                          0 001 ┊        0.006 ┊        0.006 ┊  ├─ random
                          1 000 ┊       63.343 ┊       63.343 ┊  ├─ repeated
                          0 001 ┊      132.841 ┊       54.091 ┊  ╰┬ nested
                          0 001 ┊        0.007 ┊        0.007 ┊   ├─ random
                          1 000 ┊       70.875 ┊       70.875 ┊   ╰─ repeated

```

## Setup

Let's assume that you already have an explicit setup for `tracing` like this, then you simply
need to add the highlighted line:

```rust
    use reqray::CallTreeCollector;
    use tracing_subscriber::{EnvFilter, util::SubscriberInitExt, fmt, prelude::*};

    let fmt_layer = fmt::layer()
        .with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    tracing_subscriber::registry()
    // -----------------------------------------------
        .with(CallTreeCollector::default())
    // -----------------------------------------------
        .with(filter_layer)
        .with(fmt_layer)
        .init();
```

Instead of `CallTreeCollector::default()` you can chose a more explicit config:

```rust
    // ...
    let call_tree_collector = CallTreeCollectorBuilder::default()
        .max_call_depth(10)
        .build_with_collector(
            LoggingCallTreeCollectorBuilder::default()
                .left_margin(20)
                .build(),
        );

    tracing_subscriber::registry()
        .with(call_tree_collector)
        // ...
```

## Inspiration

When working together with Klas Kalass, he created something similar for Java:
the [Fuava CTProfiler](https://github.com/freiheit-com/fuava_ctprofiler).

It proved to be immensely useful at a nearly daily basis. Thank you, Klas!

Since then, I have worked with sophisticated distributed tracing systems,
but they often lacked aggregation possibilities. Others hacked some interesting
aggregation scripts on top and I myself became somewhat obsessed with creating
similar scripts.

## Thanks

I felt very welcome when I suggested something like this in issue
[tracing#639](https://github.com/tokio-rs/tracing/issues/639). Thank you, @hawkw!

Similarly, Eliza was very supportive and helpful in the tracing discod channel.
Thank you, Eliza!

## Contributions

Contributions in the form of documentation and bug fixes are highly welcome.
Please start a discussion with me before working on larger features.

I'd really appreciate tests for all new features. Please run `cargo test`
before submitting a pull request. Just use `cargo fmt` for formatting.

Feature ideas are also welcome -- just know that this is a pure hobby side
project and I will not allocate a lot of bandwidth to this. Therefore, important
bug fixes are always prioritised.

By submitting a pull request, you agree to license your changes via all the
current licenses of the project.
