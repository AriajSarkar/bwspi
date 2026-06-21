//! Bit-Width Sparse Pointer Index (BWSPI), also called Sarkar Bucket Array.
//!
//! The implementation lives in [`sarkar_bucket_array`].  The root exports are
//! retained so existing `use bwspi::Bwspi` callers continue to compile.

pub mod sarkar_bucket_array;

pub use sarkar_bucket_array::{bit_width, BucketStats, Bwspi, SarkarSortStats};

/// SIMD bucket scanning is optional and always has a scalar fallback.
pub mod simd_search {
    pub use crate::sarkar_bucket_array::simd::simd_contains;
}
