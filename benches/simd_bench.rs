//! # SIMD + SwissTable Benchmark Suite
//!
//! Compares BWSPI search with SIMD acceleration against:
//! - BWSPI scalar search (baseline)
//! - `hashbrown::HashMap` (Google SwissTable — same impl backing std HashMap,
//!   but used directly for exact version control: v0.17.1)
//! - `std::collections::HashMap` (SipHash, SwissTable internally)
//! - `AHashMap` (aHash + SwissTable)
//! - `FxHashMap` (FxHash — fastest integer hasher)
//! - Linear search (raw Vec<u64> scan)
//!
//! ## Benchmark Dimensions
//!
//! - **Lookup HIT** — search for an element that exists (cold + hot)
//! - **Lookup MISS** — search for an element that does NOT exist
//! - **Data distributions** — high entropy, uniform width (BWSPI worst case),
//!   sequential IDs, clustered ranges
//! - **Scale** — 1K, 10K, 100K, 1M elements
//!
//! ## Running
//!
//! ```sh
//! RUSTFLAGS="-C target-cpu=native" cargo bench --bench simd_bench
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use ahash::AHashMap;
use hashbrown::HashMap as SwissMap;
use rustc_hash::FxHashMap;
use std::collections::HashMap;

use bwspi::{simd_search::simd_contains, Bwspi};

// ── Data Generators ────────────────────────────────────────────────────

fn gen_high_entropy(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    (0..n)
        .map(|_| {
            let bits = rng.gen_range(0..=64u32);
            if bits == 0 {
                0
            } else if bits == 64 {
                rng.gen::<u64>()
            } else {
                let min = 1u64 << (bits - 1);
                let max = if bits < 64 {
                    (1u64 << bits) - 1
                } else {
                    u64::MAX
                };
                rng.gen_range(min..=max)
            }
        })
        .collect()
}

fn gen_uniform_width(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    (0..n)
        .map(|_| rng.gen_range((1u64 << 31)..=(1u64 << 32) - 1))
        .collect()
}

fn gen_sequential(n: usize) -> Vec<u64> {
    (1..=n as u64).collect()
}

fn gen_clustered(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let clusters = [
        (100u64, 200),
        (10_000, 20_000),
        (1_000_000, 2_000_000),
        (1u64 << 40, (1u64 << 40) + 100_000),
        (1u64 << 60, (1u64 << 60) + 100_000),
    ];
    (0..n)
        .map(|_| {
            let c = &clusters[rng.gen_range(0..clusters.len())];
            rng.gen_range(c.0..=c.1)
        })
        .collect()
}

fn gen_data(dist: &str, n: usize) -> Vec<u64> {
    match dist {
        "high_entropy" => gen_high_entropy(n, 42),
        "uniform_width" => gen_uniform_width(n, 42),
        "sequential" => gen_sequential(n),
        "clustered" => gen_clustered(n, 42),
        _ => unreachable!(),
    }
}

// ── Benchmark: Lookup HIT ──────────────────────────────────────────────

fn bench_lookup_hit(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("simd_hit/{}", dist));

        for &size in &sizes {
            let data = gen_data(dist, size);
            let target = data[size / 2]; // guaranteed hit

            group.throughput(Throughput::Elements(1));

            // ── BWSPI + SIMD ──
            let bwspi = {
                let mut b = Bwspi::with_capacity(size);
                b.insert_bulk(&data);
                b
            };
            group.bench_with_input(BenchmarkId::new("bwspi_simd", size), &target, |b, &t| {
                b.iter(|| black_box(simd_contains(&bwspi, black_box(t))));
            });

            // ── BWSPI scalar ──
            group.bench_with_input(BenchmarkId::new("bwspi_scalar", size), &target, |b, &t| {
                b.iter(|| black_box(bwspi.contains(black_box(t))));
            });

            // ── hashbrown SwissTable (Google SwissTable v0.17) ──
            let swiss: SwissMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("swisstable", size), &target, |b, &t| {
                b.iter(|| black_box(swiss.get(&black_box(t))));
            });

            // ── HashMap (std — SipHash + SwissTable) ──
            let std_map: HashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("hashmap_std", size), &target, |b, &t| {
                b.iter(|| black_box(std_map.get(&black_box(t))));
            });

            // ── AHashMap (aHash + SwissTable) ──
            let ahash_map: AHashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("ahash", size), &target, |b, &t| {
                b.iter(|| black_box(ahash_map.get(&black_box(t))));
            });

            // ── FxHashMap (FxHash — fastest integer hasher) ──
            let fx_map: FxHashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("fxhash", size), &target, |b, &t| {
                b.iter(|| black_box(fx_map.get(&black_box(t))));
            });

            // ── Linear search (Vec<u64> scan) ──
            group.bench_with_input(BenchmarkId::new("linear_scan", size), &target, |b, &t| {
                b.iter(|| black_box(data.iter().any(|&v| v == black_box(t))));
            });
        }
        group.finish();
    }
}

// ── Benchmark: Lookup MISS ─────────────────────────────────────────────

fn bench_lookup_miss(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("simd_miss/{}", dist));

        for &size in &sizes {
            let data = gen_data(dist, size);
            let target = u64::MAX - 7; // extremely unlikely to exist

            group.throughput(Throughput::Elements(1));

            let bwspi = {
                let mut b = Bwspi::with_capacity(size);
                b.insert_bulk(&data);
                b
            };

            // ── BWSPI + SIMD ──
            group.bench_with_input(BenchmarkId::new("bwspi_simd", size), &target, |b, &t| {
                b.iter(|| black_box(simd_contains(&bwspi, black_box(t))));
            });

            // ── BWSPI scalar ──
            group.bench_with_input(BenchmarkId::new("bwspi_scalar", size), &target, |b, &t| {
                b.iter(|| black_box(bwspi.contains(black_box(t))));
            });

            // ── hashbrown SwissTable ──
            let swiss: SwissMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("swisstable", size), &target, |b, &t| {
                b.iter(|| black_box(swiss.get(&black_box(t))));
            });

            // ── HashMap (std) ──
            let std_map: HashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("hashmap_std", size), &target, |b, &t| {
                b.iter(|| black_box(std_map.get(&black_box(t))));
            });

            // ── AHashMap ──
            let ahash_map: AHashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("ahash", size), &target, |b, &t| {
                b.iter(|| black_box(ahash_map.get(&black_box(t))));
            });

            // ── FxHashMap ──
            let fx_map: FxHashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("fxhash", size), &target, |b, &t| {
                b.iter(|| black_box(fx_map.get(&black_box(t))));
            });

            // ── Linear scan ──
            group.bench_with_input(BenchmarkId::new("linear_scan", size), &target, |b, &t| {
                b.iter(|| black_box(data.iter().any(|&v| v == black_box(t))));
            });
        }
        group.finish();
    }
}

// ── Benchmark: Insertion (include SwissTable) ──────────────────────────

fn bench_insertion(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];

    let mut group = c.benchmark_group("simd_insert/high_entropy");
    for &size in &sizes {
        let data = gen_high_entropy(size, 42);
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("bwspi", size), &data, |b, data| {
            b.iter(|| {
                let mut bwspi = Bwspi::with_capacity(data.len());
                for &v in data {
                    bwspi.insert(black_box(v));
                }
                black_box(&bwspi);
            });
        });

        group.bench_with_input(BenchmarkId::new("swisstable", size), &data, |b, data| {
            b.iter(|| {
                let mut m = SwissMap::with_capacity(data.len());
                for (i, &v) in data.iter().enumerate() {
                    m.insert(black_box(v), i);
                }
                black_box(&m);
            });
        });

        group.bench_with_input(BenchmarkId::new("hashmap_std", size), &data, |b, data| {
            b.iter(|| {
                let mut m = HashMap::with_capacity(data.len());
                for (i, &v) in data.iter().enumerate() {
                    m.insert(black_box(v), i);
                }
                black_box(&m);
            });
        });

        group.bench_with_input(BenchmarkId::new("ahash", size), &data, |b, data| {
            b.iter(|| {
                let mut m = AHashMap::with_capacity(data.len());
                for (i, &v) in data.iter().enumerate() {
                    m.insert(black_box(v), i);
                }
                black_box(&m);
            });
        });

        group.bench_with_input(BenchmarkId::new("fxhash", size), &data, |b, data| {
            b.iter(|| {
                let mut m: FxHashMap<u64, usize> = FxHashMap::default();
                m.reserve(data.len());
                for (i, &v) in data.iter().enumerate() {
                    m.insert(black_box(v), i);
                }
                black_box(&m);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_lookup_hit,
    bench_lookup_miss,
    bench_insertion,
);
criterion_main!(benches);
