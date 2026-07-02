//! # HashSet vs BWSPI — Feature-Parity Benchmark
//!
//! Tests ONLY the operations that `std::collections::HashSet` provides,
//! using the exact same operations on BWSPI. No extras from either side.
//!
//! ## Operations Benchmarked (HashSet feature set only)
//!
//! | Operation         | HashSet             | BWSPI                |
//! |-------------------|---------------------|----------------------|
//! | Insert one        | `.insert(v)`        | `.insert(v)`         |
//! | Bulk insert       | `.extend(iter)`     | `.insert_bulk(&v)`   |
//! | Contains (hit)    | `.contains(&v)`     | `.contains(v)`       |
//! | Contains (miss)   | `.contains(&v)`     | `.contains(v)`       |
//! | Remove            | `.remove(&v)`       | `.remove(v)`         |
//! | Len               | `.len()`            | `.len()`             |
//! | Iterate all       | `.iter()`           | `.iter()`            |
//!
//! ## Data Distributions
//!
//! - High-entropy (random bit-widths across full u64 range)
//! - Uniform-width (BWSPI worst case — all values same bit-width)
//! - Sequential (1..=N, database PKs)
//! - Clustered (5 tight ranges, simulates telemetry hot spots)
//!
//! ## Scales: 1K, 10K, 100K, 1M

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;

use bwspi::Bwspi;

// ─── Data Generators ─────────────────────────────────────────────────

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
                let max = 1u64 << bits;
                let min = if bits > 0 { 1u64 << (bits - 1) } else { 0 };
                rng.gen_range(min..max)
            }
        })
        .collect()
}

fn gen_uniform_width(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let lo = 1u64 << 31;
    let hi = (1u64 << 32) - 1;
    (0..n).map(|_| rng.gen_range(lo..=hi)).collect()
}

fn gen_sequential(n: usize) -> Vec<u64> {
    (1..=n as u64).collect()
}

fn gen_clustered(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let clusters: Vec<(u64, u64)> = vec![
        (100, 200),
        (10_000, 20_000),
        (1_000_000, 2_000_000),
        (1u64 << 40, (1u64 << 40) + 100_000),
        (1u64 << 60, (1u64 << 60) + 100_000),
    ];
    (0..n)
        .map(|_| {
            let cluster = &clusters[rng.gen_range(0..clusters.len())];
            rng.gen_range(cluster.0..=cluster.1)
        })
        .collect()
}

fn gen_data(dist: &str, n: usize, seed: u64) -> Vec<u64> {
    match dist {
        "high_entropy" => gen_high_entropy(n, seed),
        "uniform_width" => gen_uniform_width(n, seed),
        "sequential" => gen_sequential(n),
        "clustered" => gen_clustered(n, seed),
        _ => unreachable!(),
    }
}

// ─── 1. Insert (one-by-one) ──────────────────────────────────────────

fn bench_insert(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("hashset_insert/{}", dist));
        for &size in &sizes {
            let data = gen_data(dist, size, 42);
            group.throughput(Throughput::Elements(size as u64));

            group.bench_with_input(BenchmarkId::new("BWSPI", size), &data, |b, data| {
                b.iter(|| {
                    let mut bwspi = Bwspi::with_capacity(data.len());
                    for &v in data {
                        bwspi.insert(black_box(v));
                    }
                    black_box(&bwspi);
                });
            });

            group.bench_with_input(BenchmarkId::new("HashSet", size), &data, |b, data| {
                b.iter(|| {
                    let mut set = HashSet::with_capacity(data.len());
                    for &v in data {
                        set.insert(black_box(v));
                    }
                    black_box(&set);
                });
            });
        }
        group.finish();
    }
}

// ─── 2. Bulk Insert ──────────────────────────────────────────────────

fn bench_bulk_insert(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("hashset_bulk_insert/{}", dist));
        for &size in &sizes {
            let data = gen_data(dist, size, 42);
            group.throughput(Throughput::Elements(size as u64));

            group.bench_with_input(BenchmarkId::new("BWSPI", size), &data, |b, data| {
                b.iter(|| {
                    let mut bwspi = Bwspi::with_capacity(data.len());
                    bwspi.insert_bulk(black_box(data));
                    black_box(&bwspi);
                });
            });

            group.bench_with_input(BenchmarkId::new("HashSet", size), &data, |b, data| {
                b.iter(|| {
                    let set: HashSet<u64> = black_box(data).iter().copied().collect();
                    black_box(&set);
                });
            });
        }
        group.finish();
    }
}

// ─── 3. Contains (hit) ──────────────────────────────────────────────

fn bench_contains_hit(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("hashset_contains_hit/{}", dist));
        for &size in &sizes {
            let data = gen_data(dist, size, 42);
            let target = data[size / 2]; // guaranteed hit

            group.throughput(Throughput::Elements(1));

            // BWSPI
            let mut bwspi = Bwspi::with_capacity(size);
            bwspi.insert_bulk(&data);
            group.bench_with_input(BenchmarkId::new("BWSPI", size), &target, |b, &t| {
                b.iter(|| black_box(bwspi.contains(black_box(t))));
            });

            // HashSet
            let set: HashSet<u64> = data.iter().copied().collect();
            group.bench_with_input(BenchmarkId::new("HashSet", size), &target, |b, &t| {
                b.iter(|| black_box(set.contains(&black_box(t))));
            });
        }
        group.finish();
    }
}

// ─── 4. Contains (miss) ─────────────────────────────────────────────

fn bench_contains_miss(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("hashset_contains_miss/{}", dist));
        for &size in &sizes {
            let data = gen_data(dist, size, 42);
            let target = u64::MAX - 7; // extremely unlikely to exist

            group.throughput(Throughput::Elements(1));

            // BWSPI
            let mut bwspi = Bwspi::with_capacity(size);
            bwspi.insert_bulk(&data);
            group.bench_with_input(BenchmarkId::new("BWSPI", size), &target, |b, &t| {
                b.iter(|| black_box(bwspi.contains(black_box(t))));
            });

            // HashSet
            let set: HashSet<u64> = data.iter().copied().collect();
            group.bench_with_input(BenchmarkId::new("HashSet", size), &target, |b, &t| {
                b.iter(|| black_box(set.contains(&black_box(t))));
            });
        }
        group.finish();
    }
}

// ─── 5. Remove ───────────────────────────────────────────────────────

fn bench_remove(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("hashset_remove/{}", dist));
        for &size in &sizes {
            let data = gen_data(dist, size, 42);
            // Remove every 100th element
            let targets: Vec<u64> = data.iter().step_by(100).copied().collect();

            // BWSPI
            group.bench_function(BenchmarkId::new("BWSPI", size), |b| {
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

            // HashSet
            group.bench_function(BenchmarkId::new("HashSet", size), |b| {
                b.iter_batched(
                    || {
                        let set: HashSet<u64> = data.iter().copied().collect();
                        set
                    },
                    |mut set| {
                        for &t in &targets {
                            black_box(set.remove(&black_box(t)));
                        }
                    },
                    criterion::BatchSize::LargeInput,
                );
            });
        }
        group.finish();
    }
}

// ─── 6. Len / is_empty (sanity — should be ~0ns for both) ───────────

fn bench_len(c: &mut Criterion) {
    let size = 100_000usize;
    let data = gen_high_entropy(size, 42);

    let mut group = c.benchmark_group("hashset_len");

    // BWSPI
    let mut bwspi = Bwspi::with_capacity(size);
    bwspi.insert_bulk(&data);
    group.bench_function("BWSPI", |b| {
        b.iter(|| {
            black_box(bwspi.len());
            black_box(bwspi.is_empty());
        });
    });

    // HashSet
    let set: HashSet<u64> = data.iter().copied().collect();
    group.bench_function("HashSet", |b| {
        b.iter(|| {
            black_box(set.len());
            black_box(set.is_empty());
        });
    });

    group.finish();
}

// ─── 7. Iterate all elements ─────────────────────────────────────────

fn bench_iterate(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];

    let mut group = c.benchmark_group("hashset_iterate");
    for &size in &sizes {
        let data = gen_high_entropy(size, 42);
        group.throughput(Throughput::Elements(size as u64));

        // BWSPI
        let mut bwspi = Bwspi::with_capacity(size);
        bwspi.insert_bulk(&data);
        group.bench_with_input(BenchmarkId::new("BWSPI", size), &size, |b, _| {
            b.iter(|| {
                let mut sum = 0u64;
                for (_idx, val) in bwspi.iter() {
                    sum = sum.wrapping_add(val);
                }
                black_box(sum);
            });
        });

        // HashSet
        let set: HashSet<u64> = data.iter().copied().collect();
        group.bench_with_input(BenchmarkId::new("HashSet", size), &size, |b, _| {
            b.iter(|| {
                let mut sum = 0u64;
                for &val in &set {
                    sum = sum.wrapping_add(val);
                }
                black_box(sum);
            });
        });
    }
    group.finish();
}

// ─── 8. Mixed workload (insert + contains interleaved) ───────────────

fn bench_mixed(c: &mut Criterion) {
    let sizes = [10_000, 100_000];
    let dists = ["high_entropy", "uniform_width"];

    for dist in &dists {
        let mut group = c.benchmark_group(format!("hashset_mixed/{}", dist));
        for &size in &sizes {
            let data = gen_data(dist, size, 42);
            let lookup_targets: Vec<u64> = data.iter().step_by(2).copied().collect();

            group.throughput(Throughput::Elements(size as u64));

            // BWSPI
            group.bench_function(BenchmarkId::new("BWSPI", size), |b| {
                b.iter(|| {
                    let mut bwspi = Bwspi::with_capacity(size);
                    let mut lt_idx = 0;
                    for (i, &v) in data.iter().enumerate() {
                        bwspi.insert(black_box(v));
                        if i % 2 == 1 {
                            black_box(bwspi.contains(black_box(
                                lookup_targets[lt_idx % lookup_targets.len()],
                            )));
                            lt_idx += 1;
                        }
                    }
                    black_box(&bwspi);
                });
            });

            // HashSet
            group.bench_function(BenchmarkId::new("HashSet", size), |b| {
                b.iter(|| {
                    let mut set = HashSet::with_capacity(size);
                    let mut lt_idx = 0;
                    for (i, &v) in data.iter().enumerate() {
                        set.insert(black_box(v));
                        if i % 2 == 1 {
                            black_box(set.contains(&black_box(
                                lookup_targets[lt_idx % lookup_targets.len()],
                            )));
                            lt_idx += 1;
                        }
                    }
                    black_box(&set);
                });
            });
        }
        group.finish();
    }
}

// ─── 9. Memory Usage Measurement (not timed — printed) ───────────────

fn bench_memory(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 100_000, 1_000_000];
    let dists = ["high_entropy", "uniform_width", "sequential", "clustered"];

    let mut group = c.benchmark_group("hashset_memory");

    for dist in &dists {
        for &size in &sizes {
            let data = gen_data(dist, size, 42);

            // BWSPI memory
            let mut bwspi = Bwspi::with_capacity(size);
            bwspi.insert_bulk(&data);
            let bwspi_bytes = bwspi.memory_usage_bytes();

            // HashSet memory (estimated):
            //   - Each entry: 8 bytes (u64 value) + ~8 bytes hash metadata
            //   - Bucket array overhead + load factor slack
            // We'll measure capacity * entry size + metadata
            let set: HashSet<u64> = data.iter().copied().collect();
            let hashset_bytes = std::mem::size_of::<HashSet<u64>>()
                + set.capacity() * (std::mem::size_of::<u64>() + 8); // value + control byte + padding

            // Use a trivial benchmark that prints the memory stats
            group.bench_function(
                BenchmarkId::new(format!("BWSPI_{}", dist), size),
                |b| {
                    b.iter(|| black_box(bwspi_bytes));
                },
            );

            group.bench_function(
                BenchmarkId::new(format!("HashSet_{}", dist), size),
                |b| {
                    b.iter(|| black_box(hashset_bytes));
                },
            );

            // Print memory comparison to stderr (visible in bench output)
            eprintln!(
                "[MEMORY] {}  n={:<10} BWSPI={:>12} bytes  HashSet≈{:>12} bytes  ratio={:.2}x",
                dist,
                size,
                bwspi_bytes,
                hashset_bytes,
                bwspi_bytes as f64 / hashset_bytes as f64,
            );
        }
    }
    group.finish();
}

// ─── Register ────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_insert,
    bench_bulk_insert,
    bench_contains_hit,
    bench_contains_miss,
    bench_remove,
    bench_len,
    bench_iterate,
    bench_mixed,
    bench_memory,
);
criterion_main!(benches);
