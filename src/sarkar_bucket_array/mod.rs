//! The Sarkar Bucket Array implementation.
//!
//! `index` owns the live-stream index. `snapshot_sort` is the independent
//! bucket-aware output sort. `sarkar_sort` is the optional in-place sort that
//! uses the existing bucket pointers as its workspace.

mod index;
mod sarkar_sort;
pub mod simd;
mod snapshot_sort;

pub use index::{bit_width, BucketStats, Bwspi};
pub use sarkar_sort::SarkarSortStats;

#[cfg(test)]
mod tests;
