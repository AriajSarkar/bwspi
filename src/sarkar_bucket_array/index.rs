use super::{sarkar_sort, snapshot_sort};

const REMOVED_POSITION: usize = usize::MAX;

/// Computes the binary bit-width of a `u64` value.
///
/// On CPUs exposing the relevant instruction this maps to a count-leading-zero
/// instruction. The Rust implementation remains portable when it does not.
#[inline(always)]
pub fn bit_width(value: u64) -> usize {
    (u64::BITS - value.leading_zeros()) as usize
}

/// Per-bucket distribution statistics for *live* values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BucketStats {
    pub bit_width: usize,
    pub count: usize,
    pub percentage: f64,
}

/// A live-stream `u64` index routed by binary bit width.
///
/// `data` retains every inserted value so an insert's storage ID is stable
/// until [`Bwspi::sarkar_sort`] is requested. Soft deletion only unlinks an ID
/// from the bucket index. `bucket_positions` makes unlinking, rebucketing, and
/// update operations O(1) after a matching ID has been located.
pub struct Bwspi {
    pub(crate) data: Vec<u64>,
    pub(crate) buckets: Vec<Vec<usize>>,
    pub(crate) sub_buckets: Vec<Option<[Vec<usize>; 8]>>,
    /// For a live storage ID: its offset in its bit-width bucket. Deleted
    /// entries use `REMOVED_POSITION`.
    pub(crate) bucket_positions: Vec<usize>,
    pub(crate) live_len: usize,
}

impl Bwspi {
    /// Creates an empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    /// Creates an empty index with capacity for storage entries.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            buckets: Vec::new(),
            sub_buckets: Vec::new(),
            bucket_positions: Vec::with_capacity(capacity),
            live_len: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn sub_index(value: u64, bw: usize) -> usize {
        (value >> bw.saturating_sub(4)) as usize & 0b111
    }

    #[inline]
    pub(crate) fn target_bucket_slice(&self, target: u64) -> &[usize] {
        let width = bit_width(target);
        if width >= self.buckets.len() {
            return &[];
        }
        if let Some(subs) = &self.sub_buckets[width] {
            &subs[Self::sub_index(target, width)]
        } else {
            &self.buckets[width]
        }
    }

    #[inline]
    fn ensure_bucket(&mut self, width: usize) {
        if width >= self.buckets.len() {
            self.buckets.resize_with(width + 1, Vec::new);
            self.sub_buckets.resize_with(width + 1, || None);
        }
    }

    #[inline]
    fn logical_bucket_is_empty(&self, width: usize) -> bool {
        if let Some(subs) = &self.sub_buckets[width] {
            subs.iter().all(|s| s.is_empty())
        } else {
            self.buckets[width].is_empty()
        }
    }

    #[inline]
    pub(crate) fn shrink_trailing(&mut self) {
        while !self.buckets.is_empty() && self.logical_bucket_is_empty(self.buckets.len() - 1) {
            self.buckets.pop();
            self.sub_buckets.pop();
        }
    }

    #[inline]
    fn link_existing(&mut self, index: usize, width: usize) {
        self.ensure_bucket(width);
        if let Some(subs) = &mut self.sub_buckets[width] {
            let s_idx = Self::sub_index(self.data[index], width);
            let bucket = &mut subs[s_idx];
            self.bucket_positions[index] = bucket.len();
            bucket.push(index);
        } else {
            let bucket = &mut self.buckets[width];
            self.bucket_positions[index] = bucket.len();
            bucket.push(index);
            if bucket.len() > 64 {
                self.split_bucket(width);
            }
        }
    }

    #[inline]
    fn split_bucket(&mut self, width: usize) {
        let mut subs: [Vec<usize>; 8] = Default::default();
        for index in self.buckets[width].drain(..) {
            let s_idx = Self::sub_index(self.data[index], width);
            self.bucket_positions[index] = subs[s_idx].len();
            subs[s_idx].push(index);
        }
        self.sub_buckets[width] = Some(subs);
    }

    /// Removes an already-validated live ID from its bucket without changing
    /// the data store or live count.
    #[inline]
    fn unlink_existing(&mut self, index: usize, width: usize) {
        let position = self.bucket_positions[index];
        debug_assert_ne!(position, REMOVED_POSITION);

        let value = self.data[index];
        let moved = if let Some(subs) = &mut self.sub_buckets[width] {
            let s_idx = Self::sub_index(value, width);
            let bucket = &mut subs[s_idx];
            debug_assert_eq!(bucket[position], index);
            let moved = *bucket.last().expect("live entry has a bucket slot");
            bucket.swap_remove(position);
            moved
        } else {
            let bucket = &mut self.buckets[width];
            debug_assert_eq!(bucket[position], index);
            let moved = *bucket.last().expect("live entry has a bucket slot");
            bucket.swap_remove(position);
            moved
        };

        if moved != index {
            self.bucket_positions[moved] = position;
        }
        self.bucket_positions[index] = REMOVED_POSITION;
    }

    // ── Live-stream CRUD ──────────────────────────────────────────────

    /// Inserts one value and returns its stable storage ID.
    #[inline]
    pub fn insert(&mut self, value: u64) -> usize {
        let index = self.data.len();
        self.data.push(value);
        self.bucket_positions.push(REMOVED_POSITION);
        self.link_existing(index, bit_width(value));
        self.live_len += 1;
        index
    }

    /// Bulk-inserts a slice.
    pub fn insert_bulk(&mut self, values: &[u64]) {
        self.data.reserve(values.len());
        self.bucket_positions.reserve(values.len());
        for &value in values {
            self.insert(value);
        }
    }

    /// Returns whether a live copy of `target` exists.
    #[inline]
    pub fn contains(&self, target: u64) -> bool {
        self.target_bucket_slice(target)
            .iter()
            .any(|&index| self.data[index] == target)
    }

    /// Returns the first live storage ID containing `target`.
    #[inline]
    pub fn find(&self, target: u64) -> Option<usize> {
        self.target_bucket_slice(target)
            .iter()
            .copied()
            .find(|&index| self.data[index] == target)
    }

    /// Returns every live storage ID containing `target`.
    #[must_use]
    pub fn find_all(&self, target: u64) -> Vec<usize> {
        self.target_bucket_slice(target)
            .iter()
            .copied()
            .filter(|&index| self.data[index] == target)
            .collect()
    }

    /// Returns the live value at `index`, if that storage ID is still active.
    #[inline]
    pub fn get(&self, index: usize) -> Option<u64> {
        (self.bucket_positions.get(index)? != &REMOVED_POSITION).then(|| self.data[index])
    }

    /// Soft-deletes the first live occurrence of `target`.
    ///
    /// The historical value remains in `data()`, but all public lookup APIs
    /// stop exposing it. The bucket unlink itself is O(1).
    pub fn remove(&mut self, target: u64) -> bool {
        let index = self
            .target_bucket_slice(target)
            .iter()
            .copied()
            .find(|&index| self.data[index] == target);
        index.is_some_and(|index| self.remove_at(index))
    }

    /// Soft-deletes an exact live storage ID in O(1).
    pub fn remove_at(&mut self, index: usize) -> bool {
        let Some(&position) = self.bucket_positions.get(index) else {
            return false;
        };
        if position == REMOVED_POSITION {
            return false;
        }

        let width = bit_width(self.data[index]);
        self.unlink_existing(index, width);
        self.live_len -= 1;
        self.shrink_trailing();
        true
    }

    /// Replaces the first live `old` value with `new`, returning whether a
    /// value was updated. This is O(k) to locate `old`, then O(1) to rebucket.
    pub fn update(&mut self, old: u64, new: u64) -> bool {
        self.find(old)
            .is_some_and(|index| self.update_at(index, new))
    }

    /// Replaces the live value at an exact storage ID.
    ///
    /// Updating inside the same bit-width bucket is one store. Crossing a
    /// bit-width boundary unlinks and relinks the existing ID in O(1), rather
    /// than rescanning the old bucket to repair its position.
    pub fn update_at(&mut self, index: usize, new: u64) -> bool {
        let Some(&position) = self.bucket_positions.get(index) else {
            return false;
        };
        if position == REMOVED_POSITION {
            return false;
        }

        let old = self.data[index];
        let old_width = bit_width(old);
        let new_width = bit_width(new);
        let needs_rebucket = if old_width != new_width {
            true
        } else if self.sub_buckets[old_width].is_some() {
            Self::sub_index(old, old_width) != Self::sub_index(new, new_width)
        } else {
            false
        };

        if !needs_rebucket {
            self.data[index] = new;
            return true;
        }

        self.unlink_existing(index, old_width);
        self.data[index] = new;
        self.link_existing(index, new_width);
        self.shrink_trailing();
        true
    }

    // ── Sorting ───────────────────────────────────────────────────────

    /// Produces an ascending, independently allocated snapshot of live data.
    ///
    /// Whole bit-width buckets are already globally ordered, so only the
    /// values collected from each individual bucket need sorting. This leaves
    /// storage IDs and the streaming index untouched.
    #[must_use]
    pub fn sorted_snapshot(&self) -> Vec<u64> {
        snapshot_sort::sorted_snapshot(self)
    }

    /// Runs Sarkar Sort over the internal storage without allocating a second
    /// data array. See [`SarkarSortStats`] for work performed.
    ///
    /// This changes physical storage order, so IDs previously returned by
    /// `insert`, `find`, or `find_all` must be treated as invalid afterwards.
    /// Use [`Bwspi::sorted_snapshot`] when stable IDs are required.
    pub fn sarkar_sort(&mut self) -> super::SarkarSortStats {
        sarkar_sort::sort_in_place(self)
    }

    /// Alias for [`Bwspi::sarkar_sort`].
    #[inline]
    pub fn sort_in_place(&mut self) -> super::SarkarSortStats {
        self.sarkar_sort()
    }

    // ── Observability ─────────────────────────────────────────────────

    /// Number of live values visible to search and sort operations.
    #[inline]
    pub fn len(&self) -> usize {
        self.live_len
    }

    /// Number of historical storage entries, including soft-deleted values.
    #[inline]
    pub fn storage_len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live_len == 0
    }

    /// Returns all historical storage, including values removed by soft delete.
    #[inline]
    pub fn data(&self) -> &[u64] {
        &self.data
    }

    /// Iterates `(storage_id, value)` pairs for live entries only.
    pub fn iter(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.data
            .iter()
            .copied()
            .enumerate()
            .filter(|(index, _)| self.bucket_positions[*index] != REMOVED_POSITION)
    }

    #[inline]
    pub fn bucket_slots(&self) -> usize {
        self.buckets.len()
    }

    pub fn active_bucket_count(&self) -> usize {
        (0..self.buckets.len())
            .filter(|&w| !self.logical_bucket_is_empty(w))
            .count()
    }

    pub fn max_bucket_size(&self) -> usize {
        (0..self.buckets.len())
            .map(|w| self.bucket_size(w))
            .max()
            .unwrap_or(0)
    }

    #[inline]
    pub fn bucket_size(&self, width: usize) -> usize {
        if let Some(subs) = &self.sub_buckets[width] {
            subs.iter().map(|s| s.len()).sum()
        } else {
            self.buckets[width].len()
        }
    }

    #[must_use]
    pub fn distribution_stats(&self) -> Vec<BucketStats> {
        let total = self.live_len as f64;
        (0..self.buckets.len())
            .filter(|&w| !self.logical_bucket_is_empty(w))
            .map(|width| BucketStats {
                bit_width: width,
                count: self.bucket_size(width),
                percentage: if total == 0.0 {
                    0.0
                } else {
                    self.bucket_size(width) as f64 / total * 100.0
                },
            })
            .collect()
    }

    /// Estimated heap bytes owned by the index. This excludes allocator
    /// metadata and includes the O(n) position map used for O(1) updates.
    #[must_use]
    pub fn memory_usage_bytes(&self) -> usize {
        let data_bytes = self.data.capacity() * std::mem::size_of::<u64>();
        let position_bytes = self.bucket_positions.capacity() * std::mem::size_of::<usize>();
        let outer_bytes = self.buckets.capacity() * std::mem::size_of::<Vec<usize>>();
        let inner_bytes: usize = self
            .buckets
            .iter()
            .map(|bucket| bucket.capacity() * std::mem::size_of::<usize>())
            .sum();
            
        let sub_outer = self.sub_buckets.capacity() * std::mem::size_of::<Option<[Vec<usize>; 8]>>();
        let sub_inner: usize = self.sub_buckets.iter()
            .filter_map(|s| s.as_ref())
            .flat_map(|arr| arr.iter())
            .map(|v| v.capacity() * std::mem::size_of::<usize>())
            .sum();

        data_bytes + position_bytes + outer_bytes + inner_bytes + sub_outer + sub_inner
    }
}

impl Default for Bwspi {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Bwspi {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Bwspi")
            .field("live_elements", &self.live_len)
            .field("storage_entries", &self.data.len())
            .field("slots", &self.bucket_slots())
            .field("active", &self.active_bucket_count())
            .field("max_bucket", &self.max_bucket_size())
            .field("heap_bytes", &self.memory_usage_bytes())
            .finish()
    }
}
