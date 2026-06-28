use super::{sarkar_sort, snapshot_sort};

const REMOVED_POSITION: usize = usize::MAX;

// ── Recursive bucket tree constants ──────────────────────────────────

const LEAF_CAP: usize = 64;
const FANOUT: usize = 8;
const FANOUT_BITS: usize = 3;

/// Computes the binary bit-width of a `u64` value.
#[inline(always)]
pub fn bit_width(value: u64) -> usize {
    (u64::BITS - value.leading_zeros()) as usize
}

#[allow(dead_code)]
#[inline(always)]
fn child_index(value: u64, bw: usize, depth: usize) -> usize {
    let shift = bw.saturating_sub(FANOUT_BITS * (depth + 1) + 1);
    (value >> shift) as usize & (FANOUT - 1)
}

// ── BucketNode (Lookup-only tree) ────────────────────────────────────
//
// Built lazily from the flat CRUD buckets. Rebuilt per-width only when
// a lookup touches a dirty width.

#[derive(Default)]
pub(crate) struct BucketNode {
    pub(crate) entries: Vec<usize>,
    pub(crate) values: Vec<u64>,
    pub(crate) fast_table: Vec<(u64, usize)>,
    pub(crate) children: Option<Box<[BucketNode; FANOUT]>>,
}

impl BucketNode {
    fn new() -> Self { Self::default() }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn find_leaf(&self, value: u64, bw: usize) -> (&[usize], &[u64]) {
        let mut node = self;
        let mut shift = bw.saturating_sub(4);
        loop {
            match &node.children {
                None => return (&node.entries, &node.values),
                Some(kids) => {
                    node = &kids[(value >> shift) as usize & (FANOUT - 1)];
                    shift = shift.saturating_sub(3);
                }
            }
        }
    }

    #[inline]
    pub(crate) fn fast_contains(&self, target: u64) -> bool {
        if self.fast_table.is_empty() { return false; }
        let mask = self.fast_table.len() - 1;
        let hash = target.wrapping_mul(0x517cc1b727220a95);
        let mut pos = ((hash ^ (hash >> 32)) as usize) & mask;
        loop {
            let (k, id) = unsafe { *self.fast_table.get_unchecked(pos) };
            if id == usize::MAX {
                return false;
            }
            if k == target {
                return true;
            }
            pos = (pos + 1) & mask;
        }
    }

    #[inline]
    pub(crate) fn fast_find(&self, target: u64) -> Option<usize> {
        if self.fast_table.is_empty() { return None; }
        let mask = self.fast_table.len() - 1;
        let hash = target.wrapping_mul(0x517cc1b727220a95);
        let mut pos = ((hash ^ (hash >> 32)) as usize) & mask;
        loop {
            let (k, id) = unsafe { *self.fast_table.get_unchecked(pos) };
            if id == usize::MAX {
                return None;
            }
            if k == target {
                return Some(id);
            }
            pos = (pos + 1) & mask;
        }
    }

    #[inline]
    pub(crate) fn fast_find_all(&self, target: u64) -> Vec<usize> {
        if self.fast_table.is_empty() { return Vec::new(); }
        let mask = self.fast_table.len() - 1;
        let hash = target.wrapping_mul(0x517cc1b727220a95);
        let mut pos = ((hash ^ (hash >> 32)) as usize) & mask;
        let mut result = Vec::new();
        loop {
            let (k, id) = unsafe { *self.fast_table.get_unchecked(pos) };
            if id == usize::MAX {
                break;
            }
            if k == target {
                result.push(id);
            }
            pos = (pos + 1) & mask;
        }
        result
    }

    fn sort_leaf(&mut self) {
        if self.entries.len() > 1 {
            let mut combined: Vec<_> = self.entries.iter().copied().zip(self.values.iter().copied()).collect();
            combined.sort_unstable_by_key(|&(_, v)| v);
            for (i, (id, val)) in combined.into_iter().enumerate() {
                self.entries[i] = id;
                self.values[i] = val;
            }
        }
    }

    /// Build tree from a flat list of storage IDs.
    fn build_from(ids: &[usize], bw: usize, data: &[u64]) -> Self {
        let cap = if ids.is_empty() { 0 } else { (ids.len() * 2).next_power_of_two().max(8) };
        let mut fast_table = vec![(u64::MAX, usize::MAX); cap];
        if cap > 0 {
            let mask = cap - 1;
            for &id in ids {
                let val = unsafe { *data.get_unchecked(id) };
                let hash = val.wrapping_mul(0x517cc1b727220a95);
                let mut pos = ((hash ^ (hash >> 32)) as usize) & mask;
                unsafe {
                    while fast_table.get_unchecked(pos).1 != usize::MAX {
                        pos = (pos + 1) & mask;
                    }
                    *fast_table.get_unchecked_mut(pos) = (val, id);
                }
            }
        }

        let mut root = BucketNode {
            entries: ids.to_vec(),
            values: ids.iter().map(|&id| data[id]).collect(),
            fast_table,
            children: None,
        };
        if root.entries.len() > LEAF_CAP {
            root.tree_split(bw, 0, data);
        } else {
            root.sort_leaf();
        }
        root
    }

    fn tree_split(&mut self, bw: usize, depth: usize, data: &[u64]) {
        if FANOUT_BITS * (depth + 1) >= bw.saturating_add(1) {
            return;
        }
        let mut kids: [BucketNode; FANOUT] = std::array::from_fn(|_| BucketNode::new());
        let old_entries = std::mem::take(&mut self.entries);
        let old_values = std::mem::take(&mut self.values);
        let shift = bw.saturating_sub(FANOUT_BITS * (depth + 1) + 1);
        for (storage_id, val) in old_entries.into_iter().zip(old_values.into_iter()) {
            let ci = (val >> shift) as usize & (FANOUT - 1);
            kids[ci].entries.push(storage_id);
            kids[ci].values.push(val);
        }
        self.children = Some(Box::new(kids));
        if let Some(kids) = &mut self.children {
            for kid in kids.iter_mut() {
                if kid.entries.len() > LEAF_CAP {
                    kid.tree_split(bw, depth + 1, data);
                } else if !kid.entries.is_empty() {
                    kid.sort_leaf();
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn total_len(&self) -> usize {
        match &self.children {
            None => self.entries.len(),
            Some(kids) => kids.iter().map(|k| k.total_len()).sum(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn all_empty(&self) -> bool {
        match &self.children {
            None => self.entries.is_empty(),
            Some(kids) => kids.iter().all(|k| k.all_empty()),
        }
    }

    pub(crate) fn for_each_leaf<F: FnMut(&Vec<usize>)>(&self, f: &mut F) {
        match &self.children {
            None => f(&self.entries),
            Some(kids) => { for kid in kids.iter() { kid.for_each_leaf(f); } }
        }
    }

    pub(crate) fn for_each_leaf_mut<F: FnMut(&mut Vec<usize>)>(&mut self, f: &mut F) {
        match &mut self.children {
            None => f(&mut self.entries),
            Some(kids) => { for kid in kids.iter_mut() { kid.for_each_leaf_mut(f); } }
        }
    }

    pub(crate) fn tree_overhead_bytes(&self) -> usize {
        let own = self.entries.capacity() * std::mem::size_of::<usize>()
            + self.values.capacity() * std::mem::size_of::<u64>()
            + self.fast_table.capacity() * std::mem::size_of::<(u64, usize)>();
        match &self.children {
            None => own,
            Some(kids) => {
                own + std::mem::size_of::<[BucketNode; FANOUT]>()
                    + kids.iter().map(|k| k.tree_overhead_bytes()).sum::<usize>()
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn has_split(&self) -> bool { self.children.is_some() }

    #[allow(dead_code)]
    pub(crate) fn max_depth(&self) -> usize {
        match &self.children {
            None => 0,
            Some(kids) => 1 + kids.iter().map(|k| k.max_depth()).max().unwrap_or(0),
        }
    }
}

// ── Per-bucket statistics ────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BucketStats {
    pub bit_width: usize,
    pub count: usize,
    pub percentage: f64,
}

// ── Bwspi (Dual-Index with Lazy Tree) ────────────────────────────────
//
//   crud_buckets — flat Vec<usize> per bit-width.
//     ALL mutations go here: O(1) push, O(1) swap_remove.
//     This is the source of truth.
//
//   trees — recursive BucketNode per bit-width.
//     Built LAZILY from crud_buckets when a lookup needs it.
//     NEVER touched during insert/remove/update.
//     Rebuilt per-width when `tree_dirty[w]` is true.
//
//   tree_dirty — one bool per bit-width.
//     Set true on any CRUD mutation. Cleared when tree is rebuilt.

pub struct Bwspi {
    pub(crate) data: Vec<u64>,

    // ── CRUD index (flat, O(1) operations) ───────────────────────
    pub(crate) crud_buckets: Vec<Vec<usize>>,
    pub(crate) bucket_positions: Vec<usize>,

    // ── Lookup index (lazy tree, ≤64 per leaf) ───────────────────
    pub(crate) trees: Vec<BucketNode>,
    pub(crate) tree_dirty: Vec<bool>,

    pub(crate) live_len: usize,
}

impl Bwspi {
    #[must_use]
    pub fn new() -> Self { Self::with_capacity(0) }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            crud_buckets: Vec::new(),
            bucket_positions: Vec::with_capacity(capacity),
            trees: Vec::new(),
            tree_dirty: Vec::new(),
            live_len: 0,
        }
    }

    #[inline]
    fn ensure_width(&mut self, width: usize) {
        if width >= self.crud_buckets.len() {
            self.crud_buckets.resize_with(width + 1, Vec::new);
            self.trees.resize_with(width + 1, BucketNode::new);
            self.tree_dirty.resize(width + 1, true);
        }
    }

    // ── Lazy tree rebuild ────────────────────────────────────────
    //
    // Only rebuilds tree[w] when a lookup needs it and it's dirty.
    // Cost: O(k) where k = crud_buckets[w].len(). Amortized over
    // all lookups until the next mutation on that width.

    #[inline]
    fn ensure_tree_clean(&mut self, width: usize) {
        if width < self.tree_dirty.len() && self.tree_dirty[width] {
            self.trees[width] = BucketNode::build_from(
                &self.crud_buckets[width], width, &self.data
            );
            self.tree_dirty[width] = false;
        }
    }

    // ── Lookup routing (uses tree, rebuilds lazily) ──────────────

    /// Ensure tree is clean, then return immutable leaf slice.
    /// Split into two steps to satisfy the borrow checker:
    /// step 1: ensure_tree_clean(&mut self)  — mutable
    /// step 2: route_clean(&self)            — immutable
    #[allow(dead_code)]
    #[inline]
    fn route_clean(&self, target: u64) -> (&[usize], &[u64]) {
        let w = bit_width(target);
        if w >= self.trees.len() { return (&[], &[]); }
        self.trees[w].find_leaf(target, w)
    }

    #[inline]
    pub(crate) fn shrink_trailing(&mut self) {
        while !self.crud_buckets.is_empty()
            && self.crud_buckets.last().is_some_and(|b| b.is_empty())
        {
            self.crud_buckets.pop();
            self.trees.pop();
            self.tree_dirty.pop();
        }
    }

    // ── CRUD (flat index only, O(1)) ─────────────────────────────
    //
    // These NEVER touch the tree. Just mark dirty.

    #[inline]
    fn crud_link(&mut self, index: usize, width: usize) {
        self.ensure_width(width);
        let bucket = &mut self.crud_buckets[width];
        self.bucket_positions[index] = bucket.len();
        bucket.push(index);
        self.tree_dirty[width] = true; // tree stale
    }

    #[inline]
    fn crud_unlink(&mut self, index: usize, width: usize) {
        let position = self.bucket_positions[index];
        debug_assert_ne!(position, REMOVED_POSITION);
        let bucket = &mut self.crud_buckets[width];
        let moved = *bucket.last().expect("live entry");
        bucket.swap_remove(position);
        if moved != index {
            self.bucket_positions[moved] = position;
        }
        self.bucket_positions[index] = REMOVED_POSITION;
        self.tree_dirty[width] = true; // tree stale
    }

    // ── Live-stream CRUD ─────────────────────────────────────────

    #[inline]
    pub fn insert(&mut self, value: u64) -> usize {
        let index = self.data.len();
        self.data.push(value);
        let width = bit_width(value);
        if width >= self.crud_buckets.len() {
            self.crud_buckets.resize_with(width + 1, Vec::new);
            self.trees.resize_with(width + 1, BucketNode::new);
            self.tree_dirty.resize(width + 1, true);
        }
        let bucket = &mut self.crud_buckets[width];
        self.bucket_positions.push(bucket.len());
        bucket.push(index);
        self.tree_dirty[width] = true;
        self.live_len += 1;
        index
    }

    pub fn insert_bulk(&mut self, values: &[u64]) {
        if values.is_empty() { return; }
        let start_idx = self.data.len();
        self.data.extend_from_slice(values);
        let mut counts = [0usize; 65];
        let mut max_width = 0;
        for &value in values {
            let w = bit_width(value);
            unsafe {
                *counts.get_unchecked_mut(w) += 1;
            }
            if w > max_width { max_width = w; }
        }
        self.ensure_width(max_width);
        let mut bucket_lens = [0usize; 65];
        let mut bucket_ptrs = [std::ptr::null_mut::<usize>(); 65];
        for w in 0..=max_width {
            let count = counts[w];
            if count > 0 {
                let bucket = &mut self.crud_buckets[w];
                let start_len = bucket.len();
                bucket_lens[w] = start_len;
                bucket.resize(start_len + count, 0);
                bucket_ptrs[w] = unsafe { bucket.as_mut_ptr().add(start_len) };
                self.tree_dirty[w] = true;
            }
        }
        self.bucket_positions.resize(start_idx + values.len(), 0);
        let pos_slice = &mut self.bucket_positions[start_idx..];
        for (i, &value) in values.iter().enumerate() {
            let w = bit_width(value);
            unsafe {
                let pos = *bucket_lens.get_unchecked(w);
                *bucket_lens.get_unchecked_mut(w) = pos + 1;
                *pos_slice.get_unchecked_mut(i) = pos;
                let ptr = *bucket_ptrs.get_unchecked(w);
                *ptr = start_idx + i;
                *bucket_ptrs.get_unchecked_mut(w) = ptr.add(1);
            }
        }
        self.live_len += values.len();
    }

    #[inline]
    pub fn contains(&mut self, target: u64) -> bool {
        let w = bit_width(target);
        self.ensure_tree_clean(w);
        if w >= self.trees.len() { return false; }
        self.trees[w].fast_contains(target)
    }

    #[inline]
    pub fn find(&mut self, target: u64) -> Option<usize> {
        let w = bit_width(target);
        self.ensure_tree_clean(w);
        if w >= self.trees.len() { return None; }
        self.trees[w].fast_find(target)
    }

    #[must_use]
    pub fn find_all(&mut self, target: u64) -> Vec<usize> {
        let w = bit_width(target);
        self.ensure_tree_clean(w);
        if w >= self.trees.len() { return Vec::new(); }
        self.trees[w].fast_find_all(target)
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<u64> {
        (self.bucket_positions.get(index)? != &REMOVED_POSITION).then(|| self.data[index])
    }

    pub fn remove(&mut self, target: u64) -> bool {
        let w = bit_width(target);
        self.ensure_tree_clean(w);
        let index = if w >= self.trees.len() { None } else { self.trees[w].fast_find(target) };
        index.is_some_and(|i| self.remove_at(i))
    }

    pub fn remove_at(&mut self, index: usize) -> bool {
        let Some(&position) = self.bucket_positions.get(index) else { return false };
        if position == REMOVED_POSITION { return false; }
        let width = bit_width(self.data[index]);
        self.crud_unlink(index, width); // O(1), marks tree dirty
        self.live_len -= 1;
        self.shrink_trailing();
        true
    }

    pub fn update(&mut self, old: u64, new: u64) -> bool {
        let w = bit_width(old);
        self.ensure_tree_clean(w);
        let index = if w >= self.trees.len() { None } else { self.trees[w].fast_find(old) };
        index.is_some_and(|i| self.update_at(i, new))
    }

    pub fn update_at(&mut self, index: usize, new: u64) -> bool {
        let Some(&position) = self.bucket_positions.get(index) else { return false };
        if position == REMOVED_POSITION { return false; }

        let old = self.data[index];
        let old_width = bit_width(old);
        let new_width = bit_width(new);

        if old_width == new_width {
            // Same width — just update the data.
            // Flat index: same bucket, same position — nothing to do.
            // Tree: mark dirty (will rebuild on next lookup).
            self.data[index] = new;
            self.tree_dirty[old_width] = true;
            return true;
        }

        // Different width — rebucket in flat index only.
        self.crud_unlink(index, old_width); // O(1)
        self.data[index] = new;
        self.crud_link(index, new_width);   // O(1)
        self.shrink_trailing();
        true
    }

    // ── Sorting ──────────────────────────────────────────────────

    #[must_use]
    pub fn sorted_snapshot(&mut self) -> Vec<u64> {
        // Ensure all trees are clean before snapshot.
        self.rebuild_all_dirty();
        snapshot_sort::sorted_snapshot(self)
    }

    pub fn sarkar_sort(&mut self) -> super::SarkarSortStats {
        self.rebuild_all_dirty();
        sarkar_sort::sort_in_place(self)
    }

    #[inline]
    pub fn sort_in_place(&mut self) -> super::SarkarSortStats { self.sarkar_sort() }

    /// Rebuild all dirty trees. Called before sort/snapshot.
    fn rebuild_all_dirty(&mut self) {
        for w in 0..self.tree_dirty.len() {
            if self.tree_dirty[w] {
                self.trees[w] = BucketNode::build_from(
                    &self.crud_buckets[w], w, &self.data
                );
                self.tree_dirty[w] = false;
            }
        }
    }

    // ── Observability ────────────────────────────────────────────

    #[inline] pub fn len(&self) -> usize { self.live_len }
    #[inline] pub fn storage_len(&self) -> usize { self.data.len() }
    #[inline] pub fn is_empty(&self) -> bool { self.live_len == 0 }
    #[inline] pub fn data(&self) -> &[u64] { &self.data }

    pub fn iter(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.data.iter().copied().enumerate()
            .filter(|(i, _)| self.bucket_positions[*i] != REMOVED_POSITION)
    }

    #[inline] pub fn bucket_slots(&self) -> usize { self.crud_buckets.len() }

    pub fn active_bucket_count(&self) -> usize {
        self.crud_buckets.iter().filter(|b| !b.is_empty()).count()
    }

    pub fn max_bucket_size(&self) -> usize {
        self.crud_buckets.iter().map(|b| b.len()).max().unwrap_or(0)
    }

    #[inline]
    pub fn bucket_size(&self, width: usize) -> usize {
        if width < self.crud_buckets.len() { self.crud_buckets[width].len() } else { 0 }
    }

    #[must_use]
    pub fn distribution_stats(&self) -> Vec<BucketStats> {
        let total = self.live_len as f64;
        (0..self.crud_buckets.len())
            .filter(|&w| !self.crud_buckets[w].is_empty())
            .map(|width| {
                let count = self.crud_buckets[width].len();
                BucketStats {
                    bit_width: width,
                    count,
                    percentage: if total == 0.0 { 0.0 } else { count as f64 / total * 100.0 },
                }
            })
            .collect()
    }

    #[must_use]
    pub fn memory_usage_bytes(&self) -> usize {
        let data_bytes = self.data.capacity() * 8;
        let pos_bytes = self.bucket_positions.capacity() * 8;
        let crud_outer = self.crud_buckets.capacity() * std::mem::size_of::<Vec<usize>>();
        let crud_inner: usize = self.crud_buckets.iter().map(|b| b.capacity() * 8).sum();
        let tree_outer = self.trees.capacity() * std::mem::size_of::<BucketNode>();
        let tree_inner: usize = self.trees.iter().map(|t| t.tree_overhead_bytes()).sum();
        let dirty_bytes = self.tree_dirty.capacity();
        data_bytes + pos_bytes + crud_outer + crud_inner + tree_outer + tree_inner + dirty_bytes
    }
}

impl Default for Bwspi {
    fn default() -> Self { Self::new() }
}

impl std::fmt::Debug for Bwspi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bwspi")
            .field("live_elements", &self.live_len)
            .field("storage_entries", &self.data.len())
            .field("slots", &self.bucket_slots())
            .field("active", &self.active_bucket_count())
            .field("max_bucket", &self.max_bucket_size())
            .field("heap_bytes", &self.memory_usage_bytes())
            .finish()
    }
}
