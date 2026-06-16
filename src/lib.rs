//! # Bit-Width Sparse Pointer Index (BWSPI)
//!
//! A zero-hash, bit-width-routed sparse indexing system for unsorted dynamic data streams.
//!
//! BWSPI uses the **binary bit-width** of each element as a zero-cost routing key.
//! `u64::leading_zeros()` compiles to a single `LZCNT` CPU instruction on modern
//! x86-64 and AArch64. Elements are partitioned into dynamically created buckets —
//! only the bit-widths present in the data get allocated, and the bucket array
//! grows and shrinks with the data.
//!
//! ## Architecture
//!
//! ```text
//! Table 1 (Data Store)          Table 2 (Bit-Width Index)
//! ┌───────────────────┐         ┌────────────────────────┐
//! │ [42, 7, 1000, 3]  │◄────────│ bw=2: [idx 3]          │  ← grows on demand
//! │  append-only Vec   │         │ bw=3: [idx 1]          │
//! │  never sorted      │         │ bw=6: [idx 0]          │
//! │  never moved       │         │ bw=10: [idx 2]         │  ← shrinks on removal
//! └───────────────────┘         └────────────────────────┘
//! ```
//!
//! - **Table 1:** Contiguous `Vec<u64>`, append-only. O(1) amortized insertion.
//! - **Table 2:** Dynamic `Vec<Vec<usize>>`. Starts empty, extends when wider
//!   values arrive, shrinks when trailing buckets empty after deletion.
//!   8-bit data → 9 slots. 32-bit data → 33 slots. Only what's needed.
//!
//! ## Complexity
//!
//! | Operation | Time                       | Space          |
//! |-----------|----------------------------|----------------|
//! | Insert    | O(1) amortized             | O(1) amortized |
//! | Search    | O(k) where k = bucket size | O(1)           |
//! | Delete    | O(k) where k = bucket size | O(1)           |
//! | Memory    | —                          | O(n) total     |

/// Computes the binary bit-width of a `u64` value.
///
/// Compiles to a single `LZCNT` instruction on x86-64 (BMI1+) and `CLZ` on AArch64.
///
/// Values sharing the same bit-width land in the same bucket:
///
/// | Values      | Binary           | Bit-Width |
/// |-------------|------------------|-----------|
/// | 0           | `0`              | 0         |
/// | 1           | `1`              | 1         |
/// | 2, 3        | `10`, `11`       | 2         |
/// | 4..7        | `100`..`111`     | 3         |
/// | 128..255    | `1000_0000`..`1111_1111` | 8 |
/// | 256..511    | `1_0000_0000`..`1_1111_1111` | 9 |
#[inline(always)]
pub fn bit_width(value: u64) -> usize {
    (64 - value.leading_zeros()) as usize
}

/// Per-bucket distribution statistics.
#[derive(Debug, Clone, Default)]
pub struct BucketStats {
    pub bit_width: usize,
    pub count: usize,
    pub percentage: f64,
}

/// The Bit-Width Sparse Pointer Index.
///
/// Routes elements by their binary bit-width into a dynamic bucket array.
/// The bucket array starts empty and only allocates slots for bit-widths
/// that actually appear in the data:
///
/// - Data `[1, 2, 5]` → 4 bucket slots (bw 0..3)
/// - Data `[100, 200, 50000]` → 17 bucket slots (bw 0..16)
/// - All 8-bit values → 9 slots, not 65
///
/// Routing cost is always **1 CPU instruction** regardless of slot count.
pub struct Bwspi {
    /// Append-only data store. Elements are never moved or sorted.
    data: Vec<u64>,

    /// Dynamic bit-width index. `buckets[b]` holds data-store indices
    /// of all elements with `bit_width(x) == b`. Length tracks the
    /// highest bit-width seen; shrinks when trailing buckets empty.
    buckets: Vec<Vec<usize>>,
}

impl Bwspi {
    /// Creates an empty BWSPI. Bucket array starts at zero slots.
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            buckets: Vec::new(),
        }
    }

    /// Creates a BWSPI with pre-allocated data store capacity.
    /// Bucket array still starts empty — bit-widths aren't known yet.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            buckets: Vec::new(),
        }
    }

    /// Extends the bucket array so that slot `bw` is valid.
    #[inline]
    fn ensure_bucket(&mut self, bw: usize) {
        if bw >= self.buckets.len() {
            self.buckets.resize_with(bw + 1, Vec::new);
        }
    }

    /// Trims trailing empty buckets to release memory.
    #[inline]
    fn shrink_trailing(&mut self) {
        while self.buckets.last().map_or(false, |b| b.is_empty()) {
            self.buckets.pop();
        }
    }

    /// Returns the bucket for `bw`, or an empty slice if out of range.
    #[inline]
    fn get_bucket(&self, bw: usize) -> &[usize] {
        if bw < self.buckets.len() {
            &self.buckets[bw]
        } else {
            &[]
        }
    }

    // ── Insertion ──────────────────────────────────────────────────────

    /// Inserts a value. O(1) amortized — no hashing, no rehashing.
    ///
    /// 1. Appends to data store
    /// 2. Computes bit-width (1 CPU instruction)
    /// 3. Grows bucket array if needed
    /// 4. Records index in the matching bucket
    #[inline]
    pub fn insert(&mut self, value: u64) {
        let index = self.data.len();
        self.data.push(value);
        let bw = bit_width(value);
        self.ensure_bucket(bw);
        self.buckets[bw].push(index);
    }

    /// Bulk-inserts a slice of values.
    pub fn insert_bulk(&mut self, values: &[u64]) {
        self.data.reserve(values.len());
        for &v in values {
            self.insert(v);
        }
    }

    // ── Search ─────────────────────────────────────────────────────────

    /// Returns `true` if `target` is found. O(k) where k = bucket size.
    /// Instantly returns `false` if the target's bit-width has no bucket.
    #[inline]
    pub fn contains(&self, target: u64) -> bool {
        let bucket = self.get_bucket(bit_width(target));
        for &idx in bucket {
            if unsafe { *self.data.get_unchecked(idx) } == target {
                return true;
            }
        }
        false
    }

    /// Returns the first data-store index of `target`, or `None`.
    #[inline]
    pub fn find(&self, target: u64) -> Option<usize> {
        let bucket = self.get_bucket(bit_width(target));
        for &idx in bucket {
            if unsafe { *self.data.get_unchecked(idx) } == target {
                return Some(idx);
            }
        }
        None
    }

    /// Returns all data-store indices matching `target`.
    pub fn find_all(&self, target: u64) -> Vec<usize> {
        let bucket = self.get_bucket(bit_width(target));
        let mut results = Vec::new();
        for &idx in bucket {
            if unsafe { *self.data.get_unchecked(idx) } == target {
                results.push(idx);
            }
        }
        results
    }

    // ── Deletion ───────────────────────────────────────────────────────

    /// Soft-deletes the first occurrence of `target` from the index.
    /// Value stays in the data store but becomes invisible to searches.
    /// Trailing empty buckets are trimmed after removal.
    pub fn remove(&mut self, target: u64) -> bool {
        let bw = bit_width(target);
        if bw >= self.buckets.len() {
            return false;
        }
        let bucket = &mut self.buckets[bw];
        if let Some(pos) = bucket.iter().position(|&idx| self.data[idx] == target) {
            bucket.swap_remove(pos);
            self.shrink_trailing();
            true
        } else {
            false
        }
    }

    // ── Accessors ──────────────────────────────────────────────────────

    #[inline]
    pub fn len(&self) -> usize { self.data.len() }

    #[inline]
    pub fn is_empty(&self) -> bool { self.data.is_empty() }

    pub fn data(&self) -> &[u64] { &self.data }

    /// Returns the data-store indices for a given bit-width bucket.
    /// Returns an empty slice if the bit-width is out of range.
    #[inline]
    pub fn bucket_indices(&self, bw: usize) -> &[usize] {
        self.get_bucket(bw)
    }

    /// Number of bucket slots currently allocated (tracks highest bit-width + 1).
    pub fn bucket_slots(&self) -> usize { self.buckets.len() }

    /// Number of non-empty buckets.
    pub fn active_bucket_count(&self) -> usize {
        self.buckets.iter().filter(|b| !b.is_empty()).count()
    }

    /// Largest bucket size (worst-case single-search cost).
    pub fn max_bucket_size(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).max().unwrap_or(0)
    }

    /// Size of a specific bit-width bucket (0 if out of range).
    pub fn bucket_size(&self, bw: usize) -> usize {
        if bw < self.buckets.len() { self.buckets[bw].len() } else { 0 }
    }

    /// Distribution statistics for all non-empty buckets.
    pub fn distribution_stats(&self) -> Vec<BucketStats> {
        let total = self.data.len() as f64;
        self.buckets.iter().enumerate()
            .filter(|(_, b)| !b.is_empty())
            .map(|(bw, b)| BucketStats {
                bit_width: bw,
                count: b.len(),
                percentage: if total > 0.0 { (b.len() as f64 / total) * 100.0 } else { 0.0 },
            })
            .collect()
    }

    /// Estimated heap memory in bytes (data store + bucket index + bucket contents).
    pub fn memory_usage_bytes(&self) -> usize {
        let data_bytes = self.data.capacity() * std::mem::size_of::<u64>();
        let outer_bytes = self.buckets.capacity() * std::mem::size_of::<Vec<usize>>();
        let inner_bytes: usize = self.buckets.iter()
            .map(|b| b.capacity() * std::mem::size_of::<usize>())
            .sum();
        data_bytes + outer_bytes + inner_bytes
    }
}

impl Default for Bwspi {
    fn default() -> Self { Self::new() }
}

impl std::fmt::Debug for Bwspi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bwspi")
            .field("elements", &self.data.len())
            .field("slots", &self.bucket_slots())
            .field("active", &self.active_bucket_count())
            .field("max_bucket", &self.max_bucket_size())
            .field("heap_bytes", &self.memory_usage_bytes())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_width_values() {
        assert_eq!(bit_width(0), 0);
        assert_eq!(bit_width(1), 1);
        assert_eq!(bit_width(2), 2);
        assert_eq!(bit_width(3), 2);
        assert_eq!(bit_width(4), 3);
        assert_eq!(bit_width(7), 3);
        assert_eq!(bit_width(8), 4);
        assert_eq!(bit_width(255), 8);
        assert_eq!(bit_width(256), 9);
        assert_eq!(bit_width(u64::MAX), 64);
        assert_eq!(bit_width(1u64 << 63), 64);
        assert_eq!(bit_width((1u64 << 63) - 1), 63);
    }

    #[test]
    fn insert_and_search() {
        let mut b = Bwspi::new();
        b.insert(42);
        b.insert(100);
        b.insert(0);
        b.insert(u64::MAX);

        assert!(b.contains(42));
        assert!(b.contains(100));
        assert!(b.contains(0));
        assert!(b.contains(u64::MAX));
        assert!(!b.contains(999));
    }

    #[test]
    fn find_returns_index() {
        let mut b = Bwspi::new();
        b.insert(10); b.insert(20); b.insert(30);

        assert_eq!(b.find(10), Some(0));
        assert_eq!(b.find(20), Some(1));
        assert_eq!(b.find(30), Some(2));
        assert_eq!(b.find(40), None);
    }

    #[test]
    fn find_all_duplicates() {
        let mut b = Bwspi::new();
        b.insert(42); b.insert(99); b.insert(42); b.insert(42);
        assert_eq!(b.find_all(42), vec![0, 2, 3]);
    }

    #[test]
    fn remove_soft_delete() {
        let mut b = Bwspi::new();
        b.insert(42); b.insert(42);

        assert!(b.remove(42));
        assert!(b.contains(42));   // one copy left
        assert!(b.remove(42));
        assert!(!b.contains(42));  // both gone from index
        assert!(!b.remove(42));    // nothing left
    }

    #[test]
    fn bucket_routing() {
        let mut b = Bwspi::new();
        b.insert(255); // bw=8
        b.insert(256); // bw=9

        assert_eq!(b.bucket_size(8), 1);
        assert_eq!(b.bucket_size(9), 1);
        assert_eq!(b.bucket_size(7), 0);
    }

    #[test]
    fn distribution_stats_sum() {
        let mut b = Bwspi::new();
        for i in 0..1000u64 { b.insert(i); }
        let total_pct: f64 = b.distribution_stats().iter().map(|s| s.percentage).sum();
        assert!((total_pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn zero_handling() {
        let mut b = Bwspi::new();
        b.insert(0); b.insert(0);
        assert_eq!(b.bucket_size(0), 2);
        assert!(b.contains(0));
        assert_eq!(b.find_all(0), vec![0, 1]);
    }

    #[test]
    fn powers_of_two_each_unique_bucket() {
        let mut b = Bwspi::new();
        for exp in 0..64u32 { b.insert(1u64 << exp); }
        assert_eq!(b.active_bucket_count(), 64);
        for exp in 0..64u32 { assert_eq!(b.bucket_size((exp + 1) as usize), 1); }
    }

    #[test]
    fn bulk_insert() {
        let mut b = Bwspi::new();
        let vals: Vec<u64> = (0..500).collect();
        b.insert_bulk(&vals);
        assert_eq!(b.len(), 500);
        assert!(b.contains(499));
        assert!(!b.contains(500));
    }

    // ── Dynamic growth / shrinkage ────────────────────────────────────

    #[test]
    fn empty_has_zero_slots() {
        let b = Bwspi::new();
        assert_eq!(b.bucket_slots(), 0);
        assert_eq!(b.active_bucket_count(), 0);
    }

    #[test]
    fn grows_on_wider_values() {
        let mut b = Bwspi::new();
        b.insert(1);       assert_eq!(b.bucket_slots(), 2);   // bw=1 → slots 0..=1
        b.insert(255);     assert_eq!(b.bucket_slots(), 9);   // bw=8 → slots 0..=8
        b.insert(u64::MAX);assert_eq!(b.bucket_slots(), 65);  // bw=64 → slots 0..=64
    }

    #[test]
    fn shrinks_on_removal() {
        let mut b = Bwspi::new();
        b.insert(1);    // bw=1
        b.insert(255);  // bw=8
        assert_eq!(b.bucket_slots(), 9);

        b.remove(255);
        assert_eq!(b.bucket_slots(), 2);  // trimmed to bw=0..1

        b.remove(1);
        assert_eq!(b.bucket_slots(), 0);  // fully empty
    }

    #[test]
    fn search_beyond_range_is_instant_false() {
        let mut b = Bwspi::new();
        b.insert(3);  // bw=2, only 3 slots exist
        assert!(!b.contains(u64::MAX));
        assert_eq!(b.find(u64::MAX), None);
        assert!(b.find_all(u64::MAX).is_empty());
    }

    #[test]
    fn eight_bit_data_uses_nine_slots() {
        let mut b = Bwspi::new();
        for i in 0..=255u64 { b.insert(i); }
        assert_eq!(b.bucket_slots(), 9);
        assert_eq!(b.len(), 256);
    }

    #[test]
    fn remove_beyond_range_returns_false() {
        let mut b = Bwspi::new();
        b.insert(5);  // bw=3 → 4 slots
        assert!(!b.remove(u64::MAX));
        assert_eq!(b.bucket_slots(), 4);
    }

    #[test]
    fn gap_buckets_are_empty() {
        let mut b = Bwspi::new();
        b.insert(1);    // bw=1
        b.insert(200);  // bw=8
        assert_eq!(b.bucket_slots(), 9);
        assert_eq!(b.active_bucket_count(), 2);
        for bw in 2..8 { assert_eq!(b.bucket_size(bw), 0); }
    }

    #[test]
    fn memory_tracks_actual_allocation() {
        let mut b = Bwspi::with_capacity(1000);
        for i in 0..1000u64 { b.insert(i); }
        assert!(b.memory_usage_bytes() > 0);
        assert!(b.memory_usage_bytes() < 1_000_000); // sanity bound
    }
}
