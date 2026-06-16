//! SIMD-accelerated search for BWSPI bucket scans.
//!
//! Uses AVX2 intrinsics to compare 4 × u64 values per cycle during
//! the localized bucket scan. Falls back to scalar search on hardware
//! without AVX2.
//!
//! ## How it works
//!
//! BWSPI routes a search target to a specific bit-width bucket (1 LZCNT
//! instruction), then must linearly scan that bucket. This module replaces
//! the scalar `for &idx in bucket` loop with a SIMD-vectorized scan:
//!
//! 1. Broadcast the target across all 4 lanes of a 256-bit register (`VPBROADCASTQ`)
//! 2. Gather 4 u64 values from the data store via bucket indices
//! 3. Compare all 4 lanes in parallel (`VPCMPEQQ` — AVX2)
//! 4. Cast to double and extract mask (`VMOVMSKPD`) — 1 bit per u64 lane
//! 5. If mask != 0, at least one match found
//! 6. Handle the remaining tail elements (< 4) with scalar code

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use bwspi::{bit_width, Bwspi};

/// SIMD-accelerated search within a BWSPI bucket.
///
/// Resolves the target's bit-width bucket, then scans the bucket's
/// data-store entries using AVX2 (4 × u64 per iteration) with scalar fallback.
pub fn simd_contains(bwspi: &Bwspi, target: u64) -> bool {
    let bw = bit_width(target);
    let bucket = bwspi.bucket_indices(bw);
    let data = bwspi.data();

    if bucket.is_empty() {
        return false;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { avx2_scan(data, bucket, target) };
        }
    }

    scalar_scan(data, bucket, target)
}

/// AVX2 vectorized scan: compares 4 bucket entries at a time.
///
/// Intrinsics used (all from `std::arch::x86_64`, verified against Rust docs):
///
/// | Intrinsic              | Instruction | What it does                       | Requires |
/// |------------------------|-------------|------------------------------------|----------|
/// | `_mm256_set1_epi64x`   | VPBROADCASTQ| Broadcast target to 4 lanes        | AVX      |
/// | `_mm256_loadu_si256`   | VMOVDQU     | Unaligned 256-bit load (4 × u64)   | AVX      |
/// | `_mm256_cmpeq_epi64`   | VPCMPEQQ    | 64-bit equality, 4 lanes parallel  | AVX2     |
/// | `_mm256_castsi256_pd`  | (bitcast)   | Zero-cost cast __m256i → __m256d   | AVX      |
/// | `_mm256_movemask_pd`   | VMOVMSKPD   | Extract MSB per f64 lane → 4 bits  | AVX      |
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_scan(data: &[u64], bucket: &[usize], target: u64) -> bool {
    // Broadcast target to all 4 lanes (note: intrinsic takes i64, bit pattern preserved)
    let needle = _mm256_set1_epi64x(target as i64);
    let len = bucket.len();
    let chunks = len / 4;
    let remainder = len % 4;

    // Process 4 bucket entries per iteration
    for i in 0..chunks {
        let base = i * 4;

        // Gather 4 data values from non-contiguous bucket indices.
        // Can't use VPGATHERQQ here because bucket indices are usize
        // pointers into the data store — manual gather into a local array,
        // then do one unaligned SIMD load from the stack-local array.
        let vals = [
            *data.get_unchecked(*bucket.get_unchecked(base)),
            *data.get_unchecked(*bucket.get_unchecked(base + 1)),
            *data.get_unchecked(*bucket.get_unchecked(base + 2)),
            *data.get_unchecked(*bucket.get_unchecked(base + 3)),
        ];

        let haystack = _mm256_loadu_si256(vals.as_ptr() as *const __m256i);
        let cmp = _mm256_cmpeq_epi64(haystack, needle);

        // _mm256_movemask_pd gives 4-bit mask (1 bit per u64 lane) —
        // much cleaner than _mm256_movemask_epi8's 32-bit byte mask
        let mask = _mm256_movemask_pd(_mm256_castsi256_pd(cmp));

        if mask != 0 {
            return true;
        }
    }

    // Scalar tail: remaining 0–3 elements
    let tail_start = chunks * 4;
    for i in 0..remainder {
        if *data.get_unchecked(*bucket.get_unchecked(tail_start + i)) == target {
            return true;
        }
    }

    false
}

/// Scalar fallback for non-x86 or missing AVX2.
#[inline]
fn scalar_scan(data: &[u64], bucket: &[usize], target: u64) -> bool {
    for &idx in bucket {
        if unsafe { *data.get_unchecked(idx) } == target {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::simd_contains;
    use bwspi::Bwspi;

    #[test]
    fn simd_matches_scalar_correctness() {
        let mut b = Bwspi::new();
        for i in 0..10_000u64 {
            b.insert(i);
        }

        // Every hit must agree
        for i in 0..10_000u64 {
            assert_eq!(simd_contains(&b, i), b.contains(i),
                "mismatch for value {}", i);
        }

        // Misses
        assert!(!simd_contains(&b, 99_999));
        assert!(!simd_contains(&b, u64::MAX));
    }

    #[test]
    fn simd_empty_index() {
        let b = Bwspi::new();
        assert!(!simd_contains(&b, 0));
        assert!(!simd_contains(&b, 42));
        assert!(!simd_contains(&b, u64::MAX));
    }

    #[test]
    fn simd_single_element() {
        let mut b = Bwspi::new();
        b.insert(42);
        assert!(simd_contains(&b, 42));
        assert!(!simd_contains(&b, 43));
    }

    #[test]
    fn simd_tail_elements_not_multiple_of_4() {
        let mut b = Bwspi::new();
        // All bit-width 7 (64..127) — bucket has 3 elements (pure tail, no SIMD chunk)
        b.insert(65);
        b.insert(66);
        b.insert(67);

        assert!(simd_contains(&b, 65));
        assert!(simd_contains(&b, 66));
        assert!(simd_contains(&b, 67));
        assert!(!simd_contains(&b, 68));
    }

    #[test]
    fn simd_exact_chunk_boundary() {
        let mut b = Bwspi::new();
        // Exactly 4 values with same bit-width (bw=7: 64..127)
        b.insert(64);
        b.insert(65);
        b.insert(66);
        b.insert(67);

        assert!(simd_contains(&b, 64));
        assert!(simd_contains(&b, 67));
        assert!(!simd_contains(&b, 63)); // bw=6, different bucket entirely
    }

    #[test]
    fn simd_large_bucket_stress() {
        let mut b = Bwspi::new();
        // 1000 values all same bit-width (32-bit) — many SIMD chunks
        for i in (1u64 << 31)..((1u64 << 31) + 1000) {
            b.insert(i);
        }

        assert!(simd_contains(&b, 1u64 << 31));
        assert!(simd_contains(&b, (1u64 << 31) + 999));
        assert!(!simd_contains(&b, (1u64 << 31) + 1000));
    }
}
