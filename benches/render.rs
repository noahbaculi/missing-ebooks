//! Criterion bench for `render::page` and the per-section render. Three
//! sizes (1k, 10k, 50k folders), depth=3 (the audiobook Author/Series/Book
//! shape), gap_rate=0.5, both view modes. Fanout grows with size so each row
//! actually hits the leaf level and `Throughput::Elements` reports honest
//! per-folder numbers. The per-folder throughput column pins the ADR-0022
//! claim. Baseline save and compare catches regressions on maud, the section
//! renderer, or `count_gaps`.

// `criterion_group!` and `criterion_main!` generate undocumented items.
#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use missing_ebooks::scanner::RootScan;
use missing_ebooks::synthetic::synthetic_root_scan;
use missing_ebooks::tree::ViewMode;
use missing_ebooks::web::package;
use missing_ebooks::web::render;

/// One bench input. Built once per size so the measured closure runs only
/// the function under test.
struct Input {
    raw: Vec<RootScan>,
}

/// `(total, label, fanout)`. Fanout per size is the smallest `N` whose
/// `N + N^2 + N^3` capacity at depth=3 covers `total`, so the seeder fills
/// the leaf level rather than capping mid-container.
///
/// - 1k:  fanout=10 (capacity 1110)
/// - 10k: fanout=22 (capacity 11154)
/// - 50k: fanout=37 (capacity 52059)
const SIZES: &[(usize, &str, usize)] =
    &[(1_000, "1k", 10), (10_000, "10k", 22), (50_000, "50k", 37)];
const DEPTH: usize = 3;
const GAP_RATE: f64 = 0.5;

fn bench_render(c: &mut Criterion) {
    for &(size, label, fanout) in SIZES {
        let scan: RootScan = synthetic_root_scan(size, DEPTH, fanout, GAP_RATE);
        let raw: Vec<RootScan> = vec![scan];
        let input = Input { raw };

        // `render::page` folds packaging inside the render seam, so this
        // measures the full raw → HTML pipeline the / handler runs.
        let mut group = c.benchmark_group("page");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("gaps", label), &input, |b, i| {
            b.iter(|| render::page(&i.raw, &[], ViewMode::GapsOnly, 0).into_string());
        });
        group.bench_with_input(BenchmarkId::new("all", label), &input, |b, i| {
            b.iter(|| render::page(&i.raw, &[], ViewMode::All, 0).into_string());
        });
        group.finish();

        // The group name stays `render_oob_section` so Criterion's baseline
        // history carries across the ADR-0034 fold that dropped OOB rendering.
        // The label is a historical anchor; the measured path is the plain
        // per-section render now used by /mark, /unmark, and the section swap.
        let mut group = c.benchmark_group("render_oob_section");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("gaps", label), &input, |b, i| {
            b.iter(|| {
                package::packaged_section(&i.raw, 0, ViewMode::GapsOnly)
                    .render(&[], None)
                    .into_string()
            });
        });
        group.bench_with_input(BenchmarkId::new("all", label), &input, |b, i| {
            b.iter(|| {
                package::packaged_section(&i.raw, 0, ViewMode::All)
                    .render(&[], None)
                    .into_string()
            });
        });
        group.finish();
    }
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
