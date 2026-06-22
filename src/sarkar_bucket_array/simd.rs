//! Optional AVX2 scan for a BWSPI bucket, with a portable scalar fallback.

use super::Bwspi;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Searches one routed bucket using AVX2 when available.
pub fn simd_contains(index: &Bwspi, target: u64) -> bool {
    let bucket = index.target_bucket_slice(target);
    if bucket.is_empty() {
        return false;
    }

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        // SAFETY: runtime feature detection precedes this AVX2 function, and
        // BWSPI bucket IDs always refer to valid data storage entries.
        return unsafe { avx2_scan(index.data(), bucket, target) };
    }

    scalar_scan(index.data(), bucket, target)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_scan(data: &[u64], bucket: &[usize], target: u64) -> bool {
    let needle = _mm256_set1_epi64x(target as i64);
    let chunks = bucket.chunks_exact(4);
    let tail = chunks.remainder();

    for ids in chunks {
        // BWSPI IDs need a gather. Staging four scalar loads makes the SIMD
        // comparison explicit; it does not claim that the source load is
        // contiguous or eliminate gather latency.
        let values = [data[ids[0]], data[ids[1]], data[ids[2]], data[ids[3]]];
        let haystack = _mm256_loadu_si256(values.as_ptr().cast::<__m256i>());
        let matches = _mm256_cmpeq_epi64(haystack, needle);
        if _mm256_movemask_pd(_mm256_castsi256_pd(matches)) != 0 {
            return true;
        }
    }
    scalar_scan(data, tail, target)
}

#[inline]
fn scalar_scan(data: &[u64], bucket: &[usize], target: u64) -> bool {
    bucket.iter().any(|&id| data[id] == target)
}
