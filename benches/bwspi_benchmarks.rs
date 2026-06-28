//! # BWSPI Comprehensive Benchmark Suite
//!
//! Compares BWSPI against:
//! - `std::collections::HashMap` (SipHash — DoS-resistant default)
//! - `AHashMap` (aHash — fast, hardware-accelerated)
//! - `FxHashMap` (rustc-hash — fastest integer hasher, zero DoS resistance)
//! - Vanilla linear search (raw `Vec<u64>`)
//!
//! ## Benchmark Dimensions
//!
//! 1. **Insertion throughput** — how fast can we ingest data?
//! 2. **Lookup (hit)** — search for an element that exists
//! 3. **Lookup (miss)** — search for an element that does NOT exist
//! 4. **Mixed workload** — interleaved insert + lookup
//! 5. **Data distributions** — high-entropy, uniform-width, clustered, sequential
//! 6. **Scale** — 1K, 10K, 100K, 1M elements

use ahash::AHashMap;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use hashbrown::HashMap as SwissMap;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rustc_hash::FxHashMap;
use std::collections::HashMap;

use bwspi::Bwspi;

// ---------------------------------------------------------------------------
// Data Generators
// ---------------------------------------------------------------------------

/// High-entropy: values spanning the full 64-bit range with mixed bit-widths.
fn gen_high_entropy(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    (0..n)
        .map(|_| {
            // Generate values with random bit-widths for maximum spread
            let bits = rng.gen_range(0..=64u32);
            if bits == 0 {
                0
            } else if bits == 64 {
                rng.gen::<u64>()
            } else {
                let max = 1u64 << bits;
                let min = if bits > 0 { 1u64 << (bits - 1) } else { 0 };
                rng.gen_range(min..max)
            }
        })
        .collect()
}

/// Uniform-width (worst case for BWSPI): all values have the same bit-width.
/// Generates random values in [2^31, 2^32 - 1] — all 32-bit numbers.
fn gen_uniform_width(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let lo = 1u64 << 31;
    let hi = (1u64 << 32) - 1;
    (0..n).map(|_| rng.gen_range(lo..=hi)).collect()
}

/// Sequential / auto-increment IDs (e.g., database primary keys).
fn gen_sequential(n: usize) -> Vec<u64> {
    (1..=n as u64).collect()
}

/// Clustered: values in a few tight ranges (simulates real telemetry with hot spots).
fn gen_clustered(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let clusters: Vec<(u64, u64)> = vec![
        (100, 200),                           // 8-bit range
        (10_000, 20_000),                     // 14-15 bit range
        (1_000_000, 2_000_000),               // 20-21 bit range
        (1u64 << 40, (1u64 << 40) + 100_000), // 40-bit range
        (1u64 << 60, (1u64 << 60) + 100_000), // 60-bit range
    ];
    (0..n)
        .map(|_| {
            let cluster = &clusters[rng.gen_range(0..clusters.len())];
            rng.gen_range(cluster.0..=cluster.1)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Linear search on a raw Vec — baseline comparison.
#[inline]
fn linear_search(data: &[u64], target: u64) -> bool {
    data.contains(&target)
}

// ---------------------------------------------------------------------------
// Benchmark Groups
// ---------------------------------------------------------------------------

fn bench_insertion(c: &mut Criterion) {
    let sizes: Vec<usize> = vec![1_000, 10_000, 100_000, 1_000_000];

    // --- High-Entropy Distribution ---
    {
        let mut group = c.benchmark_group("insertion/high_entropy");
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

            group.bench_with_input(BenchmarkId::new("hashmap_std", size), &data, |b, data| {
                b.iter(|| {
                    let mut map = HashMap::with_capacity(data.len());
                    for (i, &v) in data.iter().enumerate() {
                        map.insert(black_box(v), i);
                    }
                    black_box(&map);
                });
            });

            group.bench_with_input(BenchmarkId::new("hashmap_ahash", size), &data, |b, data| {
                b.iter(|| {
                    let mut map = AHashMap::with_capacity(data.len());
                    for (i, &v) in data.iter().enumerate() {
                        map.insert(black_box(v), i);
                    }
                    black_box(&map);
                });
            });

            group.bench_with_input(BenchmarkId::new("hashmap_fx", size), &data, |b, data| {
                b.iter(|| {
                    let mut map: FxHashMap<u64, usize> = FxHashMap::default();
                    map.reserve(data.len());
                    for (i, &v) in data.iter().enumerate() {
                        map.insert(black_box(v), i);
                    }
                    black_box(&map);
                });
            });

            group.bench_with_input(BenchmarkId::new("swisstable", size), &data, |b, data| {
                b.iter(|| {
                    let mut map = SwissMap::with_capacity(data.len());
                    for (i, &v) in data.iter().enumerate() {
                        map.insert(black_box(v), i);
                    }
                    black_box(&map);
                });
            });

            group.bench_with_input(BenchmarkId::new("vec_append", size), &data, |b, data| {
                b.iter(|| {
                    let mut vec = Vec::with_capacity(data.len());
                    for &v in data {
                        vec.push(black_box(v));
                    }
                    black_box(&vec);
                });
            });
        }
        group.finish();
    }

    // --- Uniform-Width Distribution (BWSPI worst case) ---
    {
        let mut group = c.benchmark_group("insertion/uniform_width");
        for &size in &sizes {
            let data = gen_uniform_width(size, 42);

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

            group.bench_with_input(BenchmarkId::new("hashmap_std", size), &data, |b, data| {
                b.iter(|| {
                    let mut map = HashMap::with_capacity(data.len());
                    for (i, &v) in data.iter().enumerate() {
                        map.insert(black_box(v), i);
                    }
                    black_box(&map);
                });
            });

            group.bench_with_input(BenchmarkId::new("hashmap_fx", size), &data, |b, data| {
                b.iter(|| {
                    let mut map: FxHashMap<u64, usize> = FxHashMap::default();
                    map.reserve(data.len());
                    for (i, &v) in data.iter().enumerate() {
                        map.insert(black_box(v), i);
                    }
                    black_box(&map);
                });
            });

            group.bench_with_input(BenchmarkId::new("vec_append", size), &data, |b, data| {
                b.iter(|| {
                    let mut vec = Vec::with_capacity(data.len());
                    for &v in data {
                        vec.push(black_box(v));
                    }
                    black_box(&vec);
                });
            });
        }
        group.finish();
    }
}

fn bench_lookup_hit(c: &mut Criterion) {
    let sizes: Vec<usize> = vec![1_000, 10_000, 100_000, 1_000_000];

    for dist_name in &["high_entropy", "uniform_width", "sequential", "clustered"] {
        let mut group = c.benchmark_group(format!("lookup_hit/{}", dist_name));

        for &size in &sizes {
            let data = match *dist_name {
                "high_entropy" => gen_high_entropy(size, 42),
                "uniform_width" => gen_uniform_width(size, 42),
                "sequential" => gen_sequential(size),
                "clustered" => gen_clustered(size, 42),
                _ => unreachable!(),
            };

            // Pick a target from the middle of the dataset (guaranteed hit)
            let target = data[size / 2];

            group.throughput(Throughput::Elements(1));

            // BWSPI
            let mut bwspi = {
                let mut b = Bwspi::with_capacity(size);
                b.insert_bulk(&data);
                b
            };
            group.bench_with_input(BenchmarkId::new("bwspi", size), &target, |b, &t| {
                b.iter(|| black_box(bwspi.contains(black_box(t))));
            });

            // HashMap (std)
            let std_map: HashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("hashmap_std", size), &target, |b, &t| {
                b.iter(|| black_box(std_map.get(&black_box(t))));
            });

            // AHashMap
            let ahash_map: AHashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("hashmap_ahash", size), &target, |b, &t| {
                b.iter(|| black_box(ahash_map.get(&black_box(t))));
            });

            // FxHashMap
            let fx_map: FxHashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("hashmap_fx", size), &target, |b, &t| {
                b.iter(|| black_box(fx_map.get(&black_box(t))));
            });

            // SwissTable (hashbrown)
            let swiss: SwissMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("swisstable", size), &target, |b, &t| {
                b.iter(|| black_box(swiss.get(&black_box(t))));
            });

            // Linear search
            group.bench_with_input(BenchmarkId::new("linear_search", size), &target, |b, &t| {
                b.iter(|| black_box(linear_search(&data, black_box(t))));
            });
        }
        group.finish();
    }
}

fn bench_lookup_miss(c: &mut Criterion) {
    let sizes: Vec<usize> = vec![1_000, 10_000, 100_000];

    for dist_name in &["high_entropy", "uniform_width"] {
        let mut group = c.benchmark_group(format!("lookup_miss/{}", dist_name));

        for &size in &sizes {
            let data = match *dist_name {
                "high_entropy" => gen_high_entropy(size, 42),
                "uniform_width" => gen_uniform_width(size, 42),
                _ => unreachable!(),
            };

            // Target that's extremely unlikely to be in the set
            let target = u64::MAX - 7;

            group.throughput(Throughput::Elements(1));

            // BWSPI
            let mut bwspi = {
                let mut b = Bwspi::with_capacity(size);
                b.insert_bulk(&data);
                b
            };
            group.bench_with_input(BenchmarkId::new("bwspi", size), &target, |b, &t| {
                b.iter(|| black_box(bwspi.contains(black_box(t))));
            });

            // HashMap (std)
            let std_map: HashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("hashmap_std", size), &target, |b, &t| {
                b.iter(|| black_box(std_map.get(&black_box(t))));
            });

            // FxHashMap
            let fx_map: FxHashMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("hashmap_fx", size), &target, |b, &t| {
                b.iter(|| black_box(fx_map.get(&black_box(t))));
            });

            // SwissTable (hashbrown)
            let swiss: SwissMap<u64, usize> =
                data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
            group.bench_with_input(BenchmarkId::new("swisstable", size), &target, |b, &t| {
                b.iter(|| black_box(swiss.get(&black_box(t))));
            });

            // Linear search
            group.bench_with_input(BenchmarkId::new("linear_search", size), &target, |b, &t| {
                b.iter(|| black_box(linear_search(&data, black_box(t))));
            });
        }
        group.finish();
    }
}

fn bench_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");
    let size = 100_000usize;
    let data = gen_high_entropy(size, 42);

    // 50% inserts, 50% lookups (alternating)
    let lookup_targets: Vec<u64> = data.iter().step_by(2).copied().collect();

    group.throughput(Throughput::Elements(size as u64));

    group.bench_function(BenchmarkId::new("bwspi", size), |b| {
        b.iter(|| {
            let mut bwspi = Bwspi::with_capacity(size);
            let mut lt_idx = 0;
            for (i, &v) in data.iter().enumerate() {
                bwspi.insert(black_box(v));
                if i % 2 == 1 {
                    black_box(
                        bwspi.contains(black_box(lookup_targets[lt_idx % lookup_targets.len()])),
                    );
                    lt_idx += 1;
                }
            }
            black_box(&bwspi);
        });
    });

    group.bench_function(BenchmarkId::new("hashmap_std", size), |b| {
        b.iter(|| {
            let mut map: HashMap<u64, usize> = HashMap::with_capacity(size);
            let mut lt_idx = 0;
            for (i, &v) in data.iter().enumerate() {
                map.insert(black_box(v), i);
                if i % 2 == 1 {
                    black_box(map.get(&black_box(lookup_targets[lt_idx % lookup_targets.len()])));
                    lt_idx += 1;
                }
            }
            black_box(&map);
        });
    });

    group.bench_function(BenchmarkId::new("hashmap_fx", size), |b| {
        b.iter(|| {
            let mut map: FxHashMap<u64, usize> = FxHashMap::default();
            map.reserve(size);
            let mut lt_idx = 0;
            for (i, &v) in data.iter().enumerate() {
                map.insert(black_box(v), i);
                if i % 2 == 1 {
                    black_box(map.get(&black_box(lookup_targets[lt_idx % lookup_targets.len()])));
                    lt_idx += 1;
                }
            }
            black_box(&map);
        });
    });

    group.bench_function(BenchmarkId::new("vec_linear", size), |b| {
        b.iter(|| {
            let mut vec = Vec::with_capacity(size);
            let mut lt_idx = 0;
            for (i, &v) in data.iter().enumerate() {
                vec.push(black_box(v));
                if i % 2 == 1 {
                    black_box(linear_search(
                        &vec,
                        black_box(lookup_targets[lt_idx % lookup_targets.len()]),
                    ));
                    lt_idx += 1;
                }
            }
            black_box(&vec);
        });
    });

    group.finish();
}

fn bench_deletion(c: &mut Criterion) {
    let mut group = c.benchmark_group("deletion");
    let size = 100_000usize;
    let data = gen_high_entropy(size, 42);
    let targets: Vec<u64> = data.iter().step_by(100).copied().collect();

    group.bench_function(BenchmarkId::new("bwspi", size), |b| {
        b.iter_batched(
            || {
                let mut bwspi = Bwspi::with_capacity(size);
                bwspi.insert_bulk(&data);
                bwspi
            },
            |mut bwspi| {
                for &t in &targets {
                    black_box(bwspi.remove(black_box(t)));
                }
            },
            criterion::BatchSize::LargeInput,
        );
    });

    group.bench_function(BenchmarkId::new("hashmap_std", size), |b| {
        b.iter_batched(
            || {
                let map: HashMap<u64, usize> =
                    data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
                map
            },
            |mut map| {
                for &t in &targets {
                    black_box(map.remove(&black_box(t)));
                }
            },
            criterion::BatchSize::LargeInput,
        );
    });

    group.bench_function(BenchmarkId::new("hashmap_fx", size), |b| {
        b.iter_batched(
            || {
                let map: FxHashMap<u64, usize> =
                    data.iter().enumerate().map(|(i, &v)| (v, i)).collect();
                map
            },
            |mut map| {
                for &t in &targets {
                    black_box(map.remove(&black_box(t)));
                }
            },
            criterion::BatchSize::LargeInput,
        );
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_insertion,
    bench_lookup_hit,
    bench_lookup_miss,
    bench_mixed_workload,
    bench_deletion,
);
criterion_main!(benches);
