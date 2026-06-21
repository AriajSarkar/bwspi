//! BWSPI — Full Production Comparison Suite
//!
//! Measures BWSPI vs HashMap vs Linear Search across:
//! - Insertion throughput
//! - Search (hit / miss)
//! - Heap memory usage (actual, not estimated)
//! - Cold run (first touch, cache-cold) vs Hot run (warmed caches)
//! - CPU routing cost (theoretical)
//! - Bucket distribution analysis
//!
//! Run: `RUSTFLAGS="-C target-cpu=native" cargo run --release`

use bwspi::{bit_width, Bwspi};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

// ── Counting Allocator ─────────────────────────────────────────────────
// Wraps the system allocator to track live heap bytes and allocation count.

struct CountingAlloc;

static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static DEALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(layout.size(), Relaxed);
        ALLOC_COUNT.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_BYTES.fetch_add(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Snapshot of heap state.
#[derive(Clone, Copy)]
struct HeapSnap {
    alloc_bytes: usize,
    alloc_count: usize,
    dealloc_bytes: usize,
}

impl HeapSnap {
    fn now() -> Self {
        Self {
            alloc_bytes: ALLOC_BYTES.load(Relaxed),
            alloc_count: ALLOC_COUNT.load(Relaxed),
            dealloc_bytes: DEALLOC_BYTES.load(Relaxed),
        }
    }

    fn live_bytes_since(&self, after: &HeapSnap) -> usize {
        let allocated = after.alloc_bytes.saturating_sub(self.alloc_bytes);
        let freed = after.dealloc_bytes.saturating_sub(self.dealloc_bytes);
        allocated.saturating_sub(freed)
    }

    fn allocs_since(&self, after: &HeapSnap) -> usize {
        after.alloc_count.saturating_sub(self.alloc_count)
    }
}

// ── Configuration ──────────────────────────────────────────────────────

const SIZES: &[usize] = &[1_000, 10_000, 100_000, 1_000_000];
const SEARCH_ITERS: usize = 10_000;
const HOT_ROUNDS: usize = 5;

// ── Data Generators ────────────────────────────────────────────────────

struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

fn gen_high_entropy(n: usize) -> Vec<u64> {
    let mut rng = Xorshift64::new(42);
    (0..n)
        .map(|_| {
            let bits = (rng.next() % 65) as u32;
            if bits == 0 {
                0
            } else if bits == 64 {
                rng.next()
            } else {
                let min = 1u64 << (bits - 1);
                let max = if bits < 64 {
                    (1u64 << bits) - 1
                } else {
                    u64::MAX
                };
                rng.range(min, max)
            }
        })
        .collect()
}

fn gen_uniform_width(n: usize) -> Vec<u64> {
    let mut rng = Xorshift64::new(42);
    (0..n)
        .map(|_| rng.range(1u64 << 31, (1u64 << 32) - 1))
        .collect()
}

fn gen_sequential(n: usize) -> Vec<u64> {
    (1..=n as u64).collect()
}

fn gen_clustered(n: usize) -> Vec<u64> {
    let mut rng = Xorshift64::new(42);
    let clusters = [
        (100u64, 200),
        (10_000, 20_000),
        (1_000_000, 2_000_000),
        (1u64 << 40, (1u64 << 40) + 100_000),
        (1u64 << 60, (1u64 << 60) + 100_000),
    ];
    (0..n)
        .map(|_| {
            let c = &clusters[(rng.next() % 5) as usize];
            rng.range(c.0, c.1)
        })
        .collect()
}

// ── Timing / Formatting ───────────────────────────────────────────────

fn time_it<F: FnOnce()>(f: F) -> Duration {
    let start = Instant::now();
    f();
    start.elapsed()
}

fn fmt_dur(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns < 1_000 {
        format!("{} ns", ns)
    } else if ns < 1_000_000 {
        format!("{:.1} µs", ns as f64 / 1e3)
    } else if ns < 1_000_000_000 {
        format!("{:.2} ms", ns as f64 / 1e6)
    } else {
        format!("{:.2} s", ns as f64 / 1e9)
    }
}

fn fmt_bytes(b: usize) -> String {
    if b < 1024 {
        format!("{} B", b)
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MB", b as f64 / (1024.0 * 1024.0))
    }
}

// ── Cold vs Hot Search ────────────────────────────────────────────────
// Cold: first pass, data likely not in CPU cache.
// Hot:  average of N subsequent passes, caches warm.

struct ColdHotResult {
    cold: Duration,
    hot_avg: Duration,
}

fn cold_hot_search<F: Fn()>(f: F) -> ColdHotResult {
    // Cold run
    let cold = time_it(&f);

    // Hot runs
    let mut total = Duration::ZERO;
    for _ in 0..HOT_ROUNDS {
        total += time_it(&f);
    }
    ColdHotResult {
        cold,
        hot_avg: total / HOT_ROUNDS as u32,
    }
}

// ── Main Comparison ───────────────────────────────────────────────────

fn run_comparison(name: &str, data: &[u64]) {
    let n = data.len();
    println!("\n{}", "═".repeat(92));
    println!("  {}  |  N = {}", name.to_uppercase(), n);
    println!("{}", "═".repeat(92));

    // ── Build structures & measure heap ──

    let snap_bwspi = HeapSnap::now();
    let mut bwspi = Bwspi::with_capacity(n);
    let bwspi_insert = time_it(|| bwspi.insert_bulk(data));
    let snap_bwspi_after = HeapSnap::now();
    let bwspi_heap = snap_bwspi.live_bytes_since(&snap_bwspi_after);
    let bwspi_allocs = snap_bwspi.allocs_since(&snap_bwspi_after);
    let bwspi_self_report = bwspi.memory_usage_bytes();

    let snap_hm = HeapSnap::now();
    let mut std_map = HashMap::with_capacity(n);
    let hm_insert = time_it(|| {
        for (i, &v) in data.iter().enumerate() {
            std_map.insert(v, i);
        }
    });
    let snap_hm_after = HeapSnap::now();
    let hm_heap = snap_hm.live_bytes_since(&snap_hm_after);
    let hm_allocs = snap_hm.allocs_since(&snap_hm_after);

    let snap_vec = HeapSnap::now();
    let mut vec_data = Vec::with_capacity(n);
    let vec_insert = time_it(|| {
        for &v in data {
            vec_data.push(v);
        }
    });
    let snap_vec_after = HeapSnap::now();
    let vec_heap = snap_vec.live_bytes_since(&snap_vec_after);
    let vec_allocs = snap_vec.allocs_since(&snap_vec_after);

    // ── Insertion ──
    println!(
        "\n  ┌───────────────────────────────────────────────────────────────────────────────┐"
    );
    println!("  │  INSERTION                                                                    │");
    println!("  ├──────────────────┬──────────────┬──────────────┬──────────────┬───────────────┤");
    println!("  │  Structure       │  Total       │  Per Elem    │  Heap Used   │  Allocations  │");
    println!("  ├──────────────────┼──────────────┼──────────────┼──────────────┼───────────────┤");
    println!(
        "  │  BWSPI           │  {:>10}  │  {:>10}  │  {:>10}  │  {:>11}  │",
        fmt_dur(bwspi_insert),
        fmt_dur(bwspi_insert / n as u32),
        fmt_bytes(bwspi_heap),
        bwspi_allocs
    );
    println!(
        "  │  HashMap (std)   │  {:>10}  │  {:>10}  │  {:>10}  │  {:>11}  │",
        fmt_dur(hm_insert),
        fmt_dur(hm_insert / n as u32),
        fmt_bytes(hm_heap),
        hm_allocs
    );
    println!(
        "  │  Vec (append)    │  {:>10}  │  {:>10}  │  {:>10}  │  {:>11}  │",
        fmt_dur(vec_insert),
        fmt_dur(vec_insert / n as u32),
        fmt_bytes(vec_heap),
        vec_allocs
    );
    println!("  └──────────────────┴──────────────┴──────────────┴──────────────┴───────────────┘");

    // ── Search targets ──
    let hit_targets: Vec<u64> = data
        .iter()
        .step_by((n / SEARCH_ITERS).max(1))
        .copied()
        .take(SEARCH_ITERS)
        .collect();
    let miss_target = u64::MAX - 13;
    let iters = hit_targets.len() as u32;

    // ── Cold / Hot search — HIT ──
    let bwspi_hit = cold_hot_search(|| {
        for &t in &hit_targets {
            std::hint::black_box(bwspi.contains(std::hint::black_box(t)));
        }
    });
    let hm_hit = cold_hot_search(|| {
        for &t in &hit_targets {
            std::hint::black_box(std_map.get(&std::hint::black_box(t)));
        }
    });
    let lin_hit = cold_hot_search(|| {
        for &t in &hit_targets {
            std::hint::black_box(vec_data.iter().any(|&v| v == std::hint::black_box(t)));
        }
    });

    // ── Cold / Hot search — MISS ──
    let bwspi_miss = cold_hot_search(|| {
        for _ in 0..SEARCH_ITERS {
            std::hint::black_box(bwspi.contains(std::hint::black_box(miss_target)));
        }
    });
    let hm_miss = cold_hot_search(|| {
        for _ in 0..SEARCH_ITERS {
            std::hint::black_box(std_map.get(&std::hint::black_box(miss_target)));
        }
    });
    let lin_miss = cold_hot_search(|| {
        for _ in 0..SEARCH_ITERS {
            std::hint::black_box(
                vec_data
                    .iter()
                    .any(|&v| v == std::hint::black_box(miss_target)),
            );
        }
    });

    println!("\n  ┌─────────────────────────────────────────────────────────────────────────────────────────┐");
    println!("  │  SEARCH ({} lookups)  —  Cold = first pass  |  Hot = avg of {} warmed passes          │",
        SEARCH_ITERS, HOT_ROUNDS);
    println!("  ├──────────────────┬───────────────────────────────┬───────────────────────────────────┤");
    println!("  │                  │  HIT (per lookup)             │  MISS (per lookup)                │");
    println!("  │  Structure       │  Cold          Hot            │  Cold           Hot               │");
    println!("  ├──────────────────┼───────────────────────────────┼───────────────────────────────────┤");
    println!(
        "  │  BWSPI           │  {:>10}     {:>10}      │  {:>10}      {:>10}          │",
        fmt_dur(bwspi_hit.cold / iters),
        fmt_dur(bwspi_hit.hot_avg / iters),
        fmt_dur(bwspi_miss.cold / iters),
        fmt_dur(bwspi_miss.hot_avg / iters)
    );
    println!(
        "  │  HashMap (std)   │  {:>10}     {:>10}      │  {:>10}      {:>10}          │",
        fmt_dur(hm_hit.cold / iters),
        fmt_dur(hm_hit.hot_avg / iters),
        fmt_dur(hm_miss.cold / iters),
        fmt_dur(hm_miss.hot_avg / iters)
    );
    println!(
        "  │  Linear Search   │  {:>10}     {:>10}      │  {:>10}      {:>10}          │",
        fmt_dur(lin_hit.cold / iters),
        fmt_dur(lin_hit.hot_avg / iters),
        fmt_dur(lin_miss.cold / iters),
        fmt_dur(lin_miss.hot_avg / iters)
    );
    println!("  └──────────────────┴───────────────────────────────┴───────────────────────────────────┘");

    // ── Memory comparison ──
    let raw = n * 8;
    println!("\n  ┌─────────────────────────────────────────────────────────────────────────┐");
    println!("  │  HEAP MEMORY (measured by counting allocator)                           │");
    println!("  ├──────────────────┬──────────────┬──────────────┬─────────────────────────┤");
    println!("  │  Structure       │  Actual Heap │  Self-Report │  Overhead vs raw data   │");
    println!("  ├──────────────────┼──────────────┼──────────────┼─────────────────────────┤");
    println!(
        "  │  BWSPI           │  {:>10}  │  {:>10}  │  {:>21.2}x  │",
        fmt_bytes(bwspi_heap),
        fmt_bytes(bwspi_self_report),
        bwspi_heap as f64 / raw as f64
    );
    println!(
        "  │  HashMap (std)   │  {:>10}  │           —  │  {:>21.2}x  │",
        fmt_bytes(hm_heap),
        hm_heap as f64 / raw as f64
    );
    println!(
        "  │  Vec (raw)       │  {:>10}  │           —  │  {:>21.2}x  │",
        fmt_bytes(vec_heap),
        vec_heap as f64 / raw as f64
    );
    println!("  └──────────────────┴──────────────┴──────────────┴─────────────────────────┘");

    // ── Bucket distribution ──
    let stats = bwspi.distribution_stats();
    println!("\n  ┌─────────────────────────────────────────────────────────────────────────┐");
    println!(
        "  │  BUCKET INDEX  ({} active / {} slots)                                  │",
        bwspi.active_bucket_count(),
        bwspi.bucket_slots()
    );
    println!("  ├──────────┬──────────┬───────────┬────────────────────────────────────────┤");
    println!("  │  Bucket  │  Count   │  % Total  │  Distribution                          │");
    println!("  ├──────────┼──────────┼───────────┼────────────────────────────────────────┤");
    for s in &stats {
        let bar_len = (s.percentage / 100.0 * 30.0).ceil() as usize;
        let bar: String = "█".repeat(bar_len.min(30));
        println!(
            "  │  bw={:<3}  │  {:>6}  │  {:>5.1}%   │  {:<37}│",
            s.bit_width, s.count, s.percentage, bar
        );
    }
    println!("  └──────────┴──────────┴───────────┴────────────────────────────────────────┘");

    let max = bwspi.max_bucket_size();
    let reduction = if n > 0 {
        (1.0 - max as f64 / n as f64) * 100.0
    } else {
        0.0
    };
    println!(
        "  📊 Worst bucket: {} / {} ({:.1}% search reduction) | {} slots allocated",
        max,
        n,
        reduction,
        bwspi.bucket_slots()
    );
}

// ── Static Analysis Tables ────────────────────────────────────────────

fn print_cpu_analysis() {
    println!("\n{}", "═".repeat(92));
    println!("  CPU ROUTING COST (per lookup)");
    println!("{}", "═".repeat(92));
    println!();
    println!(
        "  ┌────────────────────────┬───────────────────────────────────────────────────────────┐"
    );
    println!(
        "  │  System                │  Instructions / Cycle Cost                                │"
    );
    println!(
        "  ├────────────────────────┼───────────────────────────────────────────────────────────┤"
    );
    println!(
        "  │  BWSPI                 │  1 instruction (LZCNT)  — single cycle, no pipeline stall│"
    );
    println!(
        "  │  HashMap (SipHash)     │  ~60-80 instructions (4 rounds of SipHash-1-3)           │"
    );
    println!(
        "  │  HashMap (aHash)       │  ~15-20 instructions (AES-NI hardware rounds)            │"
    );
    println!(
        "  │  HashMap (FxHash)      │  ~5-8 instructions (multiply + shift)                    │"
    );
    println!(
        "  │  Linear Search         │  0 instructions (no routing — but scans everything)      │"
    );
    println!(
        "  └────────────────────────┴───────────────────────────────────────────────────────────┘"
    );
    println!();
    println!(
        "  ┌────────────────────────┬───────────────────────────────────────────────────────────┐"
    );
    println!(
        "  │  System                │  Worst-Case Resize Behavior                              │"
    );
    println!(
        "  ├────────────────────────┼───────────────────────────────────────────────────────────┤"
    );
    println!(
        "  │  BWSPI                 │  Bucket Vec grows (simple memcpy, never rehashes)        │"
    );
    println!(
        "  │  HashMap (any)         │  O(n) — rehash every key + realloc table                 │"
    );
    println!(
        "  │  Vec (linear)          │  O(n) — realloc copies entire data array                 │"
    );
    println!(
        "  └────────────────────────┴───────────────────────────────────────────────────────────┘"
    );
}

fn print_tradeoffs() {
    println!("\n{}", "═".repeat(92));
    println!("  ARCHITECTURAL COMPARISON");
    println!("{}", "═".repeat(92));
    println!();
    println!(
        "  ┌──────────────────────────┬────────────┬────────────┬────────────┬──────────────┐"
    );
    println!(
        "  │  Property                │  BWSPI     │  HashMap   │  SwissTab  │  Linear Scan │"
    );
    println!(
        "  ├──────────────────────────┼────────────┼────────────┼────────────┼──────────────┤"
    );
    println!(
        "  │  Routing cost            │  1 cycle   │  60-80 cyc │  15-20 cyc │  0           │"
    );
    println!(
        "  │  Index size              │  Dynamic   │  Dynamic   │  Dynamic   │  None        │"
    );
    println!(
        "  │  Memory overhead         │  ~2x       │  ~3-4x     │  ~2-3x     │  1.0x        │"
    );
    println!(
        "  │  Worst-case insert       │  O(1)*     │  O(n)      │  O(n)      │  O(1)*       │"
    );
    println!(
        "  │  Search (best)           │  O(1)      │  O(1)      │  O(1)      │  O(n)        │"
    );
    println!(
        "  │  Search (worst)          │  O(n)      │  O(n)      │  O(n)      │  O(n)        │"
    );
    println!(
        "  │  Rehash latency spikes   │  NEVER     │  YES       │  YES       │  NEVER       │"
    );
    println!(
        "  │  Duplicate support       │  Native    │  Overwrites│  Overwrites│  Native      │"
    );
    println!(
        "  │  Cache-line friendly     │  YES       │  Moderate  │  YES       │  YES         │"
    );
    println!(
        "  └──────────────────────────┴────────────┴────────────┴────────────┴──────────────┘"
    );
    println!("  * O(1) amortized — Vec realloc is a simple memcpy, not a full rehash.");
}

// ── Entry Point ───────────────────────────────────────────────────────

fn main() {
    println!();
    println!(
        "╔════════════════════════════════════════════════════════════════════════════════════╗"
    );
    println!(
        "║  BWSPI — Bit-Width Sparse Pointer Index — Production Comparison Suite            ║"
    );
    println!(
        "║  Measuring: Speed · Heap · Allocations · Cold/Hot Cache · Distribution           ║"
    );
    println!(
        "╚════════════════════════════════════════════════════════════════════════════════════╝"
    );

    // Verify bit_width
    println!("\n  🔬 bit_width verification:");
    for &(val, expected) in &[
        (0u64, 0),
        (1, 1),
        (2, 2),
        (3, 2),
        (255, 8),
        (256, 9),
        (u64::MAX, 64),
    ] {
        let ok = if bit_width(val) == expected {
            "✓"
        } else {
            "✗"
        };
        println!("    {} bit_width({}) = {}", ok, val, bit_width(val));
    }

    for &size in SIZES {
        run_comparison(
            &format!("High Entropy (n={})", size),
            &gen_high_entropy(size),
        );
        run_comparison(
            &format!("Uniform 32-bit [BWSPI worst case] (n={})", size),
            &gen_uniform_width(size),
        );
        run_comparison(
            &format!("Sequential IDs (n={})", size),
            &gen_sequential(size),
        );
        run_comparison(
            &format!("Clustered / 5 ranges (n={})", size),
            &gen_clustered(size),
        );
    }

    print_cpu_analysis();
    print_tradeoffs();

    println!("\n  ✅ Done. Run `cargo bench` for statistically rigorous criterion benchmarks.\n");
}
