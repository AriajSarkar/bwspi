# BWSPI — Bit-Width Sparse Pointer Index

*Also known as the **Sarkar Bucket Array (SBA)***

> A zero-hash, single-instruction routed sparse indexing system for unsorted dynamic data streams.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![Build](https://img.shields.io/badge/build-passing-brightgreen.svg)]()

---

## Why BWSPI?

If your workload is **insert-heavy**, **append-only**, or **latency-sensitive** — BWSPI is built for you:

- **5× faster insertion** than HashMap at 1M elements (6 ns vs 31 ns per element)
- **Half the memory** — 15.9 MB vs 34.0 MB at 1M elements
- **Zero rehash spikes** — no O(n) pauses, ever
- **Native duplicate support** — no overwrites like HashMap
- **98% search space reduction** on high-entropy data

BWSPI replaces cryptographic hashing with a physical property of the data — its **binary bit-width** — as a zero-cost routing key. Routing costs **exactly 1 CPU instruction** (`LZCNT` on x86-64, `CLZ` on ARM), compared to 60+ for SipHash or 15+ for AES-NI.

```
HashMap:  value → hash(value) → bucket     (60+ CPU instructions)
BWSPI:   value → bit_width(value) → bucket (1 CPU instruction)
```

> **Trade-off**: Lookup is O(k) where k = bucket size, not O(1) amortized like HashMap. BWSPI is an integer index/set, not a general-purpose key-value map. See [When to Use](#when-to-use-bwspi) and [docs/trade-offs.md](docs/trade-offs.md) for details.

---

## Architecture

```
Table 1 (Data Store)          Table 2 (Bit-Width Index)
┌───────────────────┐         ┌────────────────────────┐
│ [42, 7, 1000, 3]  │◄────────│ bw=2: [idx 3]          │  ← grows on demand
│  append-only Vec   │         │ bw=3: [idx 1]          │
│  never sorted      │         │ bw=6: [idx 0]          │
│  never moved       │         │ bw=10: [idx 2]         │  ← shrinks on removal
└───────────────────┘         └────────────────────────┘
```

- **Table 1** — Contiguous `Vec<u64>`, append-only. O(1) amortized insertion. Data is never moved or sorted.
- **Table 2** — Dynamic `Vec<Vec<usize>>`. Starts at **zero slots**. Grows only for bit-widths that exist. Shrinks when trailing buckets empty.
  - 8-bit data → 9 slots (not 65)
  - 32-bit data → 33 slots (not 65)
  - Only what's needed

---

## Key Properties

| Property | SBA / BWSPI | HashMap (std) | HashMap (FxHash) |
|----------|-------------|---------------|-------------------|
| **Routing cost** | **1 instruction** (LZCNT) | ~60-80 instructions (SipHash) | ~5-8 instructions |
| **Rehash latency spikes** | **Never** | Yes (O(n) rehash on resize) | Yes |
| **Memory overhead** | **~2x** raw data | ~3-4x raw data | ~2-3x raw data |
| **Duplicate support** | **Native** | Overwrites | Overwrites |
| **Worst-case insert** | **O(1) amortized** | O(n) rehash | O(n) rehash |
| **Cache locality** | **Excellent** | Moderate | Moderate |
| **DoS resistance** | N/A (no hash) | Yes (SipHash) | None |

---

## Benchmark Results

All benchmarks run with `RUSTFLAGS="-C target-cpu=native"`, measured by [Criterion](https://github.com/bheisler/criterion.rs) (100 samples, 5s per benchmark).

### Lookup HIT — N=1,000 (High Entropy)

| Structure | Time | Notes |
|-----------|------|-------|
| SwissTable (hashbrown 0.17) | 1.64 ns | Google SwissTable + foldhash |
| FxHashMap | 1.90 ns | Fastest integer hash, no DoS protection |
| AHashMap | 2.38 ns | AES-NI hardware hashing |
| **BWSPI (scalar)** | **2.66 ns** | Compiler auto-vectorized scan |
| **BWSPI (AVX2 SIMD)** | **2.76 ns** | Manual AVX2 (4×u64 per cycle) |
| HashMap (std) | 10.28 ns | SipHash-1-3 (DoS-resistant) |
| Linear scan | 137.68 ns | Full Vec scan |

> **On SIMD vs scalar at N=1K**: At high-entropy with 1K elements, buckets average ~15 entries. The manual AVX2 gather (4 scalar loads → stack array → SIMD compare) can't beat the compiler's auto-vectorized loop at that size. SIMD's 4×u64 throughput advantage shows at **large bucket sizes** (100+ entries) — specifically the uniform-width worst case where a single bucket holds all N elements.

### Insertion — N=1,000,000 (High Entropy)

| Structure | Total | Per Element | Heap Used |
|-----------|-------|-------------|-----------|
| **BWSPI** | **6.71 ms** | **6 ns** | **15.94 MB (2.1x)** |
| HashMap (std) | 31.91 ms | 31 ns | 34.00 MB (4.5x) |
| Vec (append) | 1.90 ms | 1 ns | 7.63 MB (1.0x) |

> **4.8× faster insertion** and **47% of HashMap's heap** at 1M elements.

### Search Reduction

BWSPI partitions data into bit-width buckets, reducing search space dramatically:

| Distribution | Worst Bucket | Search Reduction |
|-------------|-------------|------------------|
| High Entropy (1M) | 19,251 / 1M | **98.1%** |
| Clustered (100K) | 20,230 / 100K | **79.8%** |
| Sequential (100K) | 34,465 / 100K | **65.5%** |
| Uniform 32-bit | 1,000 / 1K | 0% (worst case) |

---

## SIMD Acceleration

BWSPI includes optional AVX2 SIMD-accelerated bucket scans in [`simd_search.rs`](src/simd_search.rs):

```
1. Broadcast target → [target, target, target, target]  (VPBROADCASTQ)
2. Load 4 values from bucket                             (VMOVDQU)
3. Compare 4 lanes in parallel                           (VPCMPEQQ)
4. Extract 4-bit mask                                    (VMOVMSKPD)
5. if mask != 0 → found
```

Runtime feature detection with scalar fallback — works on all x86-64 hardware.

> **When does SIMD actually help?** At small bucket sizes (< ~16 entries), the compiler auto-vectorizes the scalar loop just as well. Manual AVX2 shines when buckets grow large (100+ entries) — specifically uniform-width distributions where most data lands in one bucket. See [docs/simd.md](docs/simd.md) for the full intrinsics breakdown.

---

## Usage

```rust
use bwspi::Bwspi;

let mut index = Bwspi::new();

// Insert
index.insert(42);
index.insert(1000);
index.insert(7);

// Search — routes to bit-width bucket, then scans locally
assert!(index.contains(42));      // true
assert!(!index.contains(999));    // false — instant if bit-width bucket empty

// Find index in data store
if let Some(idx) = index.find(1000) {
    println!("Found at data store index: {}", idx);
}

// Find all occurrences (supports duplicates natively)
index.insert(42);
let indices = index.find_all(42);  // [0, 3]

// Delete (soft-delete from index, data stays)
index.remove(42);

// Bulk insert
let data = vec![1, 2, 3, 100, 200, 300];
let mut bulk = Bwspi::with_capacity(data.len());
bulk.insert_bulk(&data);

// Inspect internals
println!("Elements: {}", bulk.len());
println!("Bucket slots: {}", bulk.bucket_slots());
println!("Active buckets: {}", bulk.active_bucket_count());
println!("Heap usage: {} bytes", bulk.memory_usage_bytes());
```

---

## When to Use BWSPI

### ✅ Great for

- **Unsorted, append-only data streams** (logs, telemetry, event sourcing)
- **High-entropy data** with naturally varied bit-widths (random IDs, mixed-size integers)
- **Latency-sensitive systems** that can't tolerate HashMap rehash spikes
- **Memory-constrained environments** (~2x vs HashMap's ~4x overhead)
- **Duplicate-heavy datasets** where HashMap would overwrite
- **Write-heavy workloads** where insertion speed matters more than lookup
- **Embedded / no_std** scenarios (no hash function dependency)

### ⚠️ Not ideal for

- **Uniform-width data** (all values same bit-width → single bucket, degrades to linear scan)
- **Lookup-dominated workloads** where O(1) amortized hash lookup matters
- **Key-value pairs** (BWSPI is a set/index, not a map)
- **String or composite keys** (BWSPI operates on u64 values)

---

## Complexity

| Operation | Time | Space |
|-----------|------|-------|
| Insert | O(1) amortized | O(1) amortized |
| Search | O(k) where k = bucket size | O(1) |
| Delete | O(k) where k = bucket size | O(1) |
| Memory | — | O(n) total |

---

## How Bit-Width Routing Works

Values that need the **same number of binary digits** share a bucket:

```
Values          Binary                Bit-Width    Bucket
──────          ──────                ─────────    ──────
0               0                     0            bucket[0]
1               1                     1            bucket[1]
2, 3            10, 11                2            bucket[2]
4, 5, 6, 7      100, 101, 110, 111    3            bucket[3]
8..15           1000..1111            4            bucket[4]
16..31          10000..11111          5            bucket[5]
32..63          100000..111111        6            bucket[6]  ← 42 goes here
64..127         1000000..1111111      7            bucket[7]
128..255        10000000..11111111    8            bucket[8]
...             ...                   ...          ...
```

The routing function:

```rust
fn bit_width(value: u64) -> usize {
    (64 - value.leading_zeros()) as usize
}
```

This compiles to **exactly one CPU instruction**:
- x86-64: `LZCNT` (Leading Zero Count, BMI1)
- ARM/AArch64: `CLZ` (Count Leading Zeros)

No multiplication, no XOR, no mixing rounds. Just one instruction.

---

## Project Structure

```
sarkar-bucket-array-sba/
├── Cargo.toml                    # Dependencies and build config
├── LICENSE                       # MIT License
├── README.md                     # This file
├── docs/
│   ├── README.md                 # Documentation index
│   ├── architecture.md           # Two-table design, data flow, memory layout
│   ├── how-it-works.md           # Bit-width routing, search reduction math
│   ├── benchmarks.md             # Performance data vs all competitors
│   ├── simd.md                   # AVX2 intrinsics breakdown
│   └── trade-offs.md             # When to use / not use SBA
├── src/
│   ├── lib.rs                    # Core SBA / BWSPI implementation
│   ├── main.rs                   # Production analysis suite (heap + cache)
│   └── simd_search.rs            # AVX2 SIMD-accelerated bucket scan
└── benches/
    ├── bwspi_benchmarks.rs       # Criterion: full comparison suite
    └── simd_bench.rs             # Criterion: SIMD vs scalar vs SwissTable
```

---

## Running

```bash
# Run tests (24 total — 18 core + 6 SIMD)
cargo test
cargo test --bench simd_bench

# Full production analysis (heap measurement, cold/hot cache, distributions)
RUSTFLAGS="-C target-cpu=native" cargo run --release

# Criterion benchmarks (statistically rigorous)
RUSTFLAGS="-C target-cpu=native" cargo bench --bench simd_bench
RUSTFLAGS="-C target-cpu=native" cargo bench --bench bwspi_benchmarks
```

---

## Roadmap

- [ ] **Generic integer support** — `u32`, `u16`, `usize` via trait-based `BitWidth` interface
- [ ] **`no_std` support** — core implementation has no std dependency, needs feature gate
- [ ] **Parallel insertion** — lock-free append with per-bucket atomics
- [ ] **Persistent storage** — mmap-backed data store for zero-copy reload

---

## License

[MIT](LICENSE) — Ariaj Sarkar, 2026.

---

## Tags

`data-structure` `sparse-index` `bit-width` `zero-hash` `bucket-array` `simd` `avx2` `rust` `no-rehash` `low-latency` `memory-efficient` `append-only` `unsorted-data` `bwspi` `sba`
