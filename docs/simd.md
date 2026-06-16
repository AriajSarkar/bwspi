# SIMD Internals

> How Sarkar Bucket Array (SBA) / BWSPI accelerates bucket scans with AVX2.

---

## Why SIMD?

After SBA routes to a bucket (1 LZCNT instruction), it must linearly scan that bucket to find the target. For small buckets (~15 elements), scalar code is fast enough. For large buckets (uniform-width data, worst case), SIMD can scan **4 elements per cycle** instead of 1.

---

## The AVX2 Pipeline

```
┌─────────────────┐
│  VPBROADCASTQ   │  Broadcast target value across 4 lanes
│  [42,42,42,42]  │
└────────┬────────┘
         │
         ▼
┌─────────────────┐  ┌─────────────────┐
│   Gather 4 u64  │  │    VMOVDQU      │
│   from bucket   │──│  Load into 256- │
│   indices       │  │  bit register   │
└────────┬────────┘  └────────┬────────┘
         │                    │
         └────────┬───────────┘
                  ▼
         ┌─────────────────┐
         │    VPCMPEQQ     │  Compare all 4 lanes in parallel
         │  64-bit equality│  Result: all-1s (match) or all-0s (no match)
         └────────┬────────┘
                  │
                  ▼
         ┌─────────────────┐
         │  CASTSI256_PD   │  Zero-cost bitcast (__m256i → __m256d)
         │    +            │
         │  VMOVMSKPD      │  Extract 4-bit mask (1 bit per u64 lane)
         └────────┬────────┘
                  │
                  ▼
         ┌─────────────────┐
         │  mask != 0 ?    │  At least one of the 4 values matched
         │  → found!       │
         └─────────────────┘
```

---

## Intrinsics Reference

All from `std::arch::x86_64`, verified against [Rust docs](https://doc.rust-lang.org/core/arch/x86_64/index.html):

| Rust Intrinsic | CPU Instruction | What It Does | Required Feature |
|---|---|---|---|
| `_mm256_set1_epi64x(val)` | VPBROADCASTQ | Copy `val` to all 4 lanes of a 256-bit register | AVX |
| `_mm256_loadu_si256(ptr)` | VMOVDQU | Unaligned load of 256 bits (4 × u64) from memory | AVX |
| `_mm256_cmpeq_epi64(a, b)` | VPCMPEQQ | Compare 4 pairs of u64 for equality. Each lane → all-1s or all-0s | **AVX2** |
| `_mm256_castsi256_pd(a)` | *(bitcast)* | Zero-cost reinterpret `__m256i` as `__m256d` | AVX |
| `_mm256_movemask_pd(a)` | VMOVMSKPD | Extract the sign bit of each f64 lane → 4-bit integer | AVX |

### Why `movemask_pd` instead of `movemask_epi8`?

- `_mm256_movemask_epi8` → 32-bit mask (1 bit per byte) — you'd get 8 bits per u64 lane
- `_mm256_movemask_pd` → 4-bit mask (1 bit per u64 lane) — much cleaner

The `castsi256_pd` is free (no instruction emitted, just a type cast for the compiler).

---

## The Gather Problem

AVX2 has `VPGATHERQQ` for hardware-scatter-gather of u64 values, but we can't use it here because:

1. Bucket indices are `usize` (potentially different from the gather index type)
2. The gather instruction has high latency (~12 cycles on Skylake)
3. For 4 elements, manual gather into a stack array + one unaligned load is faster

```rust
// Manual gather: load 4 values from non-contiguous data store positions
let vals = [
    data[bucket[base]],
    data[bucket[base + 1]],
    data[bucket[base + 2]],
    data[bucket[base + 3]],
];
let haystack = _mm256_loadu_si256(vals.as_ptr() as *const __m256i);
```

---

## Runtime Feature Detection

SBA never assumes AVX2 is available. It checks at runtime:

```rust
pub fn simd_contains(bwspi: &Bwspi, target: u64) -> bool {
    if is_x86_feature_detected!("avx2") {
        return unsafe { avx2_scan(data, bucket, target) };
    }
    scalar_scan(data, bucket, target)  // fallback
}
```

The `is_x86_feature_detected!` macro uses `CPUID` on first call, then caches the result. Subsequent calls are a simple boolean check.

---

## Tail Handling

Bucket sizes are rarely multiples of 4. The remaining 0-3 elements are handled with scalar code:

```rust
let chunks = len / 4;      // full SIMD iterations
let remainder = len % 4;   // scalar tail

// SIMD loop: process 4 at a time
for i in 0..chunks { ... }

// Scalar tail: process remaining 0-3
for i in 0..remainder {
    if data[bucket[tail_start + i]] == target { return true; }
}
```

---

## SSE4.1 Note

For 128-bit (2 × u64 per cycle), you'd use `_mm_cmpeq_epi64` — but this requires **SSE4.1**, not SSE2. SSE2 has no native 64-bit integer comparison. Since AVX2 is widely available on any CPU from 2013+, we target AVX2 directly.

---

## When SIMD Helps

| Bucket Size | SIMD vs Scalar | Why |
|---|---|---|
| 1-3 elements | ~Same | Pure tail, no SIMD chunk |
| 4-15 elements | ~Same | 1-3 SIMD iterations, compiler auto-vectorizes similarly |
| 16-100 elements | **SIMD faster** | Multiple SIMD iterations amortize setup cost |
| 100+ elements | **SIMD much faster** | 4× throughput on the hot loop |

At N=1K with high entropy, buckets average ~15 elements — compiler auto-vectorization matches manual SIMD. The manual AVX2 path shines in the **uniform-width worst case** where a single bucket holds all N elements.
