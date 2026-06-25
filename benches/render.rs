//! Criterion bench for `render_view` and the per-section OOB render. Three
//! sizes (1k, 10k, 50k folders) at a single shape, both view modes. The
//! per-folder throughput column pins the ADR-0022 claim. Baseline save and
//! compare catches regressions on maud, the section renderer, or
//! `count_gaps`.

// `criterion_group!` and `criterion_main!` generate undocumented items.
#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use missing_ebooks::scanner::RootScan;
use missing_ebooks::synthetic::synthetic_root_scan;
use missing_ebooks::tree::ViewMode;
use missing_ebooks::web::render::{
    FlaggedView, package_section, package_view, render_view, single_oob_section,
};

/// One bench input. Built once per size so the measured closure runs only
/// the function under test.
struct Input {
    scan: RootScan,
    view_gaps: FlaggedView,
    view_all: FlaggedView,
}

const SIZES: &[(usize, &str)] = &[(1_000, "1k"), (10_000, "10k"), (50_000, "50k")];
const DEPTH: usize = 5;
const FANOUT: usize = 10;
const GAP_RATE: f64 = 0.5;

fn bench_render(c: &mut Criterion) {
    for &(size, label) in SIZES {
        let scan = synthetic_root_scan(size, DEPTH, FANOUT, GAP_RATE);
        let raw = vec![scan.clone()];
        let view_gaps = package_view(&raw, ViewMode::GapsOnly);
        let view_all = package_view(&raw, ViewMode::All);
        let input = Input {
            scan,
            view_gaps,
            view_all,
        };

        let mut group = c.benchmark_group("render_view");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("gaps", label), &input, |b, i| {
            b.iter(|| render_view(&i.view_gaps, &[], ViewMode::GapsOnly).into_string());
        });
        group.bench_with_input(BenchmarkId::new("all", label), &input, |b, i| {
            b.iter(|| render_view(&i.view_all, &[], ViewMode::All).into_string());
        });
        group.finish();

        let mut group = c.benchmark_group("render_oob_section");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("gaps", label), &input, |b, i| {
            b.iter(|| {
                let section = package_section(&i.scan, ViewMode::GapsOnly);
                single_oob_section(&section, 0, &[], ViewMode::GapsOnly).into_string()
            });
        });
        group.bench_with_input(BenchmarkId::new("all", label), &input, |b, i| {
            b.iter(|| {
                let section = package_section(&i.scan, ViewMode::All);
                single_oob_section(&section, 0, &[], ViewMode::All).into_string()
            });
        });
        group.finish();
    }
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
