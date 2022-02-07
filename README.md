# Reqray

[![Build Status](https://api.travis-ci.com/kolloch/reqray.svg?branch=main&status=started)](https://travis-ci.com/kolloch/reqray)
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
2022-02-06T20:01:57.103747Z  INFO Call summary of request@examples/nested.rs:51

                        # calls │   ∑ alive ms │    ∑ busy ms │ ∑ own busy ms │ span tree
                    ────────────┼──────────────┼──────────────┼────────────-──┼───────────────────────
                          0 001 ┊      258.910 ┊      258.890 ┊         0.106 ┊ ┬ request
                          0 001 ┊       87.204 ┊       87.190 ┊        19.299 ┊ ├┬ nested
                          0 001 ┊        0.036 ┊        0.021 ┊         0.021 ┊ ┊├─ random
                          1 000 ┊       75.738 ┊       61.912 ┊        61.912 ┊ ┊╰─ repeated
                          0 002 ┊        0.051 ┊        0.027 ┊         0.027 ┊ ├─ repeated
                          0 001 ┊        1.644 ┊        1.632 ┊         0.019 ┊ ├┬ nest_deeply
                          0 001 ┊        1.619 ┊        1.607 ┊         0.025 ┊ ┊╰┬ nest_deeply
                          0 001 ┊        1.593 ┊        1.577 ┊         0.024 ┊ ┊ ╰┬ nest_deeply
                          0 001 ┊        1.561 ┊        1.547 ┊         0.022 ┊ ┊  ╰┬ nest_deeply
                          0 001 ┊        1.532 ┊        1.520 ┊         0.023 ┊ ┊   ╰┬ nest_deeply
                          0 001 ┊        1.504 ┊        1.492 ┊         0.023 ┊ ┊    ╰┬ nest_deeply
                          0 001 ┊        1.476 ┊        1.463 ┊         0.025 ┊ ┊     ╰┬ nest_deeply
                          0 001 ┊        1.446 ┊        1.433 ┊         0.025 ┊ ┊      ╰┬ nest_deeply
                          0 001 ┊        1.415 ┊        1.402 ┊         1.402 ┊ ┊       ╰─ nest_deeply
                          0 001 ┊      169.915 ┊      169.905 ┊        17.883 ┊ ╰┬ nested2
                          0 001 ┊        0.010 ┊        0.001 ┊         0.001 ┊  ├─ random
                          1 000 ┊       88.793 ┊       76.081 ┊        76.081 ┊  ├─ repeated
                          0 001 ┊       70.386 ┊       70.377 ┊        19.332 ┊  ╰┬ nested
                          0 001 ┊        0.011 ┊        0.001 ┊         0.001 ┊   ├─ random
                          1 000 ┊       58.468 ┊       45.280 ┊        45.280 ┊   ╰─ repeated
```

* **calls**: The total number of spans created at this call path.
* **∑ alive ms**: The total time spans at this call path were alive i.e. sum of times between new and close events.
* **∑ busy ms**: The total time spans at this call path were entered i.e. sum of times between enter and leave events.
* **∑ own busy ms**: The total time spans at this call path were entered without any children entered.

It looked like this until 0.3.x:

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

For a quick startm, add/edit these `[dependencies]` in `Cargo.toml`:

```
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["registry", "env-filter"] }
reqray = "0.4"
```

And add/edit your tracing layer setup:

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

## Compatibility with `tracing-subscriber 0.2`

Use reqray 0.2.x for integration with tracing-subscriber 0.2.x. Otherwise, the API
should be identical.

E.g. `color_eyre` 0.5.x depends on `tracing-error` 0.1.x which requires `tracing-subscriber` 0.2. 

## Overhead

I did basic performance testing (see benches) to check for obvious gotchas
-- I didn't spot any. If your code actually does talk to a database
or anything expensive, it should be in the same order of magnitude as logging
overhead with the tracing library in general.

In my totally unrepresentative example with some log statements which does
nothing else really, the logging overhead increased by 30-50% -- this is roughly
the amount of actual log lines added to the log output in this case.

Generally, you should only instrument relevant calls in your program not every
one of them, especially not those in a CPU-bound loop. If you have those,
it might make sense to filter those before the CallTreeCollector is invoked.

I am very curious to hear actual stats in real life programs!

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

Giving feedback or saying thanks on [Twitter](https://twitter.com/pkolloch) or
on the tracing discord channel is appreciated.

Contributions in the form of documentation and bug fixes are highly welcome.
Please start a discussion with me (e.g. via an issue) before working on larger
features.

I'd really appreciate tests for all new features. Please run `cargo test`
before submitting a pull request. Just use `cargo fmt` for formatting.

Feature ideas are also welcome -- just know that this is a pure hobby side
project and I will not allocate a lot of bandwidth to this. Therefore, important
bug fixes are always prioritised.

By submitting a pull request, you agree to license your changes via all the
current licenses of the project.
