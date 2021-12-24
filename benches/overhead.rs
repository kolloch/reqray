use std::{fs::File, sync::{Arc, Mutex}};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use quanta::Mock;
use reqray::{CallTreeCollector, CallTreeCollectorBuilder, FinishedCallTreeProcessor};

use tracing::info;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

#[tracing::instrument]
fn one_ns(mock: &Mock) {
    info!("one_ns");
    mock.increment(1);
}

#[tracing::instrument]
fn compound_call(mock: &Mock) {
    mock.increment(10);
    one_ns(mock);
    mock.increment(100);
    one_ns(mock);
    for _ in 0..10 {
      one_ns(mock);
    }
    mock.increment(1000);
}

pub fn sync_compound(c: &mut Criterion) {
    let (_clock, mock) = quanta::Clock::mock();
    c.bench_function("without subscriber", |b| {
        b.iter(|| compound_call(black_box(&mock)))
    });
    c.bench_function("log with layers", |b| {
        let f = File::create("benches_without_calltree.txt").unwrap();
        let fmt_layer = fmt::layer()
            .with_thread_ids(true)
            .without_time()
            .with_target(false)
            .with_writer(f);
        let subscriber = tracing_subscriber::registry().with(fmt_layer);
        tracing::subscriber::with_default(subscriber, || {
            b.iter(|| compound_call(black_box(&mock)))
        });
    });
    c.bench_function("log with call tree collector", |b| {
        let call_tree_collector = CallTreeCollector::default();
        let f = File::create("benches_with_calltree.txt").unwrap();
        let fmt_layer = fmt::layer()
            .with_thread_ids(true)
            .without_time()
            .with_target(false)
            .with_writer(f);
        let subscriber = tracing_subscriber::registry()
            .with(call_tree_collector)
            .with(fmt_layer);
        tracing::subscriber::with_default(subscriber, || {
            b.iter(|| compound_call(black_box(&mock)))
        });
    });

    c.bench_function("log with silent call tree collector", |b| {
        let counting = CountingCallTreeProcessor::default();
        let call_tree_collector = CallTreeCollectorBuilder::default()
          .build_with_collector(counting.clone());
        let f = File::create("benches_with_silent_calltree.txt").unwrap();
        let fmt_layer = fmt::layer()
            .with_thread_ids(true)
            .without_time()
            .with_target(false)
            .with_writer(f);
        let subscriber = tracing_subscriber::registry()
            .with(call_tree_collector)
            .with(fmt_layer);
        tracing::subscriber::with_default(subscriber, || {
            b.iter(|| compound_call(black_box(&mock)))
        });
        assert!(*counting.root_child_count.lock().unwrap() > 0);
    });
  }

#[derive(Default, Clone)]
struct CountingCallTreeProcessor {
  root_child_count: Arc<Mutex<usize>>,
}

impl FinishedCallTreeProcessor for CountingCallTreeProcessor {
    fn process_finished_call(&self, pool: reqray::CallPathPool) {
        let mut locked = self.root_child_count.lock().unwrap();
        *locked += pool.root().children().count();
    }
}

criterion_group!(benches, sync_compound);
criterion_main!(benches);
