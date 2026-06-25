//! BWSPI lookup acceleration.
//!
//! With the recursive bucket tree, every leaf holds ≤64 raw storage
//! indices.  The scan is short enough that a simple scalar loop is
//! competitive with SIMD.  We keep the AVX2 path for potential future
//! use but the primary path is scalar.

use super::index::Bwspi;

/// Searches the target leaf for an exact value match.
///
/// With ≤64 entries per leaf, this scan averages ~32 comparisons.
pub fn simd_contains(index: &mut Bwspi, target: u64) -> bool {
    // contains() already handles lazy tree rebuild internally.
    index.contains(target)
}
