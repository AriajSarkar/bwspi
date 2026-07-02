use super::{sarkar_sort, snapshot_sort};

// ── LSB Radix Tree constants ─────────────────────────────────────────

const LEAF_CAP: usize = 64;
const DUPLICATE_GROUP_THRESHOLD: usize = 128;
const PENDING_RECENT_SCAN: usize = 64;
const PENDING_MERGE_THRESHOLD: usize = 1024;
const REMOVED_POSITION: usize = usize::MAX;
const NO_DUPLICATE_HINT: usize = usize::MAX;

/// Computes the binary bit-width of a `u64` value.
#[inline(always)]
pub fn bit_width(value: u64) -> usize {
    (u64::BITS - value.leading_zeros()) as usize
}

// ── FlatBucket (dense LSB radix tree) ────────────────────────────────
//
// Built lazily from the flat CRUD buckets. Rebuilt per-width only when
// a lookup touches a dirty width.
//
// Architecture:
//   - Routes by TRAILING bits (LSB), not leading bits (MSB)
//   - Each level consumes the NEXT SPLIT_BITS bits moving leftward:
//       Level 0: bits [0..SPLIT_BITS)
//       Level 1: bits [SPLIT_BITS..2*SPLIT_BITS)
//       Level 2: bits [2*SPLIT_BITS..3*SPLIT_BITS)
//       ...
//   - Children indexed by (value >> bit_offset) & mask — direct bit extraction
//   - Existing children are found through a fixed 64-slot child map
//   - Leaves store only storage_ids (Vec<usize>), no value copies
//   - Confirms match via data[storage_id] == target
//   - ZERO hash collisions, ZERO empty slots, ZERO stored values
//   - Splits progressively deeper only when a leaf exceeds LEAF_CAP
//   - Prunes empty children after build

/// Fixed number of bits consumed per tree level.
/// 6 bits = 64 possible children per node, matching the ≤64 leaf scan cap.
const SPLIT_BITS: u8 = 6;
const SPLIT_FANOUT: usize = 1 << SPLIT_BITS; // 64

pub(crate) struct FlatNode {
    /// Storage IDs at leaf level. Empty for internal nodes.
    pub(crate) entries: Vec<usize>,

    /// Bit offset: which bits this node routes by.
    /// Routes by bits [bit_offset .. bit_offset + SPLIT_BITS).
    pub(crate) bit_offset: u8,

    /// Bit i is 1 when child i exists. Zero means this node is a leaf.
    pub(crate) child_bitmap: u64,

    /// Start index of this node's 64 child slots inside FlatBucket.child_slots.
    pub(crate) children_start: usize,

    /// Cursor for large exact-duplicate leaves. `NO_DUPLICATE_HINT` means this
    /// is a normal leaf.
    duplicate_hint: usize,
}

impl Default for FlatNode {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            bit_offset: 0,
            child_bitmap: 0,
            children_start: 0,
            duplicate_hint: NO_DUPLICATE_HINT,
        }
    }
}

#[derive(Default)]
pub(crate) struct FlatBucket {
    pub(crate) nodes: Vec<FlatNode>,
    pub(crate) child_slots: Vec<u32>,
}

impl FlatBucket {
    fn new() -> Self {
        Self::default()
    }

    // ── LSB Radix Lookup Methods ─────────────────────────────────
    //
    // Each level extracts SPLIT_BITS bits starting at self.bit_offset.
    // Level 0: idx = (value >> 0)  & 0x3F → bits 0-5
    // Level 1: idx = (value >> 6)  & 0x3F → bits 6-11
    // Level 2: idx = (value >> 12) & 0x3F → bits 12-17

    #[inline]
    fn leaf_node_index_for(&self, target: u64) -> Option<usize> {
        let mut node_idx = 0;
        loop {
            let node = self.nodes.get(node_idx)?;
            if node.child_bitmap == 0 {
                return Some(node_idx);
            }

            let child_pos = ((target >> node.bit_offset) as usize) & (SPLIT_FANOUT - 1);
            let bit = 1_u64 << child_pos;
            if node.child_bitmap & bit == 0 {
                return None;
            }

            let child_slot = node.children_start + child_pos;
            let child_idx = *self.child_slots.get(child_slot)?;
            if child_idx == u32::MAX {
                return None;
            }
            node_idx = child_idx as usize;
        }
    }

    #[inline]
    fn leaf_entries_for(&self, target: u64) -> Option<&[usize]> {
        let node_idx = self.leaf_node_index_for(target)?;
        Some(&self.nodes[node_idx].entries)
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn lsb_contains(&self, target: u64, data: &[u64]) -> bool {
        self.leaf_entries_for(target).is_some_and(|entries| {
            entries
                .binary_search_by_key(&target, |&id| data[id])
                .is_ok()
        })
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn lsb_find(&self, target: u64, data: &[u64]) -> Option<usize> {
        let entries = self.leaf_entries_for(target)?;
        entries
            .binary_search_by_key(&target, |&id| data[id])
            .ok()
            .map(|position| entries[position])
    }

    // ── Surgical Remove (no tree rebuild) ─────────────────────────
    //
    // Traverses the tree to find the leaf containing `target`,
    // removes the entry directly from the leaf's sorted entries,
    // and prunes the parent's bitmap if the child becomes empty.
    //
    // This keeps the tree VALID without a full rebuild.
    // Cost: O(tree_depth + leaf_size) — typically O(log N).

    /// Remove one occurrence of `target` from the tree in-place.
    /// Returns the storage_id if found, `None` otherwise.
    #[allow(dead_code)]
    pub(crate) fn surgical_remove(&mut self, target: u64, data: &[u64]) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }

        let mut node_idx = 0;
        // Track (parent_node_idx, child_pos_in_parent) for pruning.
        let mut parent_info: Option<(usize, usize)> = None;

        loop {
            let bitmap = self.nodes[node_idx].child_bitmap;

            if bitmap == 0 {
                // Leaf node — remove the entry directly.
                let entries = &mut self.nodes[node_idx].entries;
                let pos = entries.binary_search_by_key(&target, |&id| data[id]).ok()?;
                let storage_id = entries[pos];
                entries.remove(pos);

                // Prune: if this leaf is now empty, clear parent's bit.
                if entries.is_empty() {
                    if let Some((parent_idx, child_pos)) = parent_info {
                        self.nodes[parent_idx].child_bitmap &= !(1u64 << child_pos);
                    }
                }

                return Some(storage_id);
            }

            // Internal node — route to child.
            let bit_offset = self.nodes[node_idx].bit_offset;
            let children_start = self.nodes[node_idx].children_start;
            let child_pos = ((target >> bit_offset) as usize) & (SPLIT_FANOUT - 1);
            let bit = 1_u64 << child_pos;

            if bitmap & bit == 0 {
                return None;
            }

            let child_slot = children_start + child_pos;
            let child_idx = *self.child_slots.get(child_slot)?;
            if child_idx == u32::MAX {
                return None;
            }

            parent_info = Some((node_idx, child_pos));
            node_idx = child_idx as usize;
        }
    }

    /// Remove a *specific* storage_id from the tree (for `remove_at` with duplicates).
    /// Navigates to the leaf, then scans all entries with the same value
    /// to find the exact storage_id.
    #[allow(dead_code)]
    pub(crate) fn surgical_remove_id(
        &mut self,
        target: u64,
        storage_id: usize,
        data: &[u64],
    ) -> bool {
        if self.nodes.is_empty() {
            return false;
        }

        let mut node_idx = 0;
        let mut parent_info: Option<(usize, usize)> = None;

        loop {
            let bitmap = self.nodes[node_idx].child_bitmap;

            if bitmap == 0 {
                // Leaf node — find the exact storage_id among duplicates.
                let entries = &mut self.nodes[node_idx].entries;

                // Find the range of entries with the target value.
                let start = entries.partition_point(|&id| data[id] < target);
                let mut found = None;
                for i in start..entries.len() {
                    if data[entries[i]] != target {
                        break;
                    }
                    if entries[i] == storage_id {
                        found = Some(i);
                        break;
                    }
                }

                let pos = match found {
                    Some(p) => p,
                    None => return false,
                };
                entries.remove(pos);

                // Prune: if this leaf is now empty, clear parent's bit.
                if entries.is_empty() {
                    if let Some((parent_idx, child_pos)) = parent_info {
                        self.nodes[parent_idx].child_bitmap &= !(1u64 << child_pos);
                    }
                }

                return true;
            }

            // Internal node — route to child.
            let bit_offset = self.nodes[node_idx].bit_offset;
            let children_start = self.nodes[node_idx].children_start;
            let child_pos = ((target >> bit_offset) as usize) & (SPLIT_FANOUT - 1);
            let bit = 1_u64 << child_pos;

            if bitmap & bit == 0 {
                return false;
            }

            let child_slot = children_start + child_pos;
            let child_idx = match self.child_slots.get(child_slot) {
                Some(&idx) if idx != u32::MAX => idx,
                _ => return false,
            };

            parent_info = Some((node_idx, child_pos));
            node_idx = child_idx as usize;
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn lsb_find_all(&self, target: u64, data: &[u64]) -> Vec<usize> {
        self.leaf_entries_for(target)
            .map(|entries| {
                let start = entries.partition_point(|&id| data[id] < target);
                let end = entries[start..].partition_point(|&id| data[id] == target) + start;
                entries[start..end].to_vec()
            })
            .unwrap_or_default()
    }

    // ── Tombstone-aware lookups ────────────────────────────────
    //
    // These skip entries where `positions[id] == REMOVED_POSITION`,
    // allowing the tree to contain dead entries (tombstones) that
    // are filtered at lookup time.

    /// Find the first LIVE entry matching `target`, skipping tombstones.
    pub(crate) fn lsb_find_live(
        &mut self,
        target: u64,
        data: &[u64],
        positions: &[usize],
    ) -> Option<usize> {
        let node_idx = self.leaf_node_index_for(target)?;
        let node = &mut self.nodes[node_idx];
        if node.duplicate_hint != NO_DUPLICATE_HINT {
            let duplicate_value = data[*node.entries.first()?];
            if duplicate_value != target {
                return None;
            }
            while node.duplicate_hint < node.entries.len() {
                let id = node.entries[node.duplicate_hint];
                if positions[id] != REMOVED_POSITION {
                    return Some(id);
                }
                node.duplicate_hint += 1;
            }
            return None;
        }

        let entries = &node.entries;
        let start = entries.partition_point(|&id| data[id] < target);
        for &id in &entries[start..] {
            if data[id] != target {
                break;
            }
            if positions[id] != REMOVED_POSITION {
                return Some(id);
            }
        }
        None
    }

    /// Find ALL live entries matching `target`, skipping tombstones.
    pub(crate) fn lsb_find_all_live(
        &mut self,
        target: u64,
        data: &[u64],
        positions: &[usize],
    ) -> Vec<usize> {
        let Some(node_idx) = self.leaf_node_index_for(target) else {
            return Vec::new();
        };
        let node = &self.nodes[node_idx];
        if node.duplicate_hint != NO_DUPLICATE_HINT {
            let Some(&first_id) = node.entries.first() else {
                return Vec::new();
            };
            if data[first_id] != target {
                return Vec::new();
            }
            return node
                .entries
                .iter()
                .copied()
                .filter(|&id| positions[id] != REMOVED_POSITION)
                .collect();
        }

        let entries = &node.entries;
        let start = entries.partition_point(|&id| data[id] < target);
        entries[start..]
            .iter()
            .take_while(|&&id| data[id] == target)
            .copied()
            .filter(|&id| positions[id] != REMOVED_POSITION)
            .collect()
    }

    /// Build a dense LSB radix tree from a flat list of storage IDs.
    fn build_from(ids: &[usize], _bw: usize, data: &[u64], bit_offset: u8) -> Self {
        if ids.is_empty() {
            return Self::default();
        }

        let mut nodes = Vec::with_capacity((ids.len() / LEAF_CAP).saturating_mul(2).max(1));
        let mut child_slots = Vec::new();
        Self::build_node(ids, data, bit_offset, &mut nodes, &mut child_slots);
        Self { nodes, child_slots }
    }

    fn build_node(
        ids: &[usize],
        data: &[u64],
        bit_offset: u8,
        nodes: &mut Vec<FlatNode>,
        child_slots: &mut Vec<u32>,
    ) -> usize {
        let node_idx = nodes.len();
        nodes.push(FlatNode::default());
        Self::fill_node(node_idx, ids, data, bit_offset, nodes, child_slots);
        node_idx
    }

    fn fill_node(
        node_idx: usize,
        ids: &[usize],
        data: &[u64],
        bit_offset: u8,
        nodes: &mut Vec<FlatNode>,
        child_slots: &mut Vec<u32>,
    ) {
        if ids.len() <= LEAF_CAP || bit_offset >= 64 {
            let mut entries = ids.to_vec();
            if entries.len() > 1 {
                entries.sort_unstable_by_key(|&id| data[id]);
            }
            nodes[node_idx] = FlatNode {
                entries,
                bit_offset,
                child_bitmap: 0,
                children_start: 0,
                duplicate_hint: NO_DUPLICATE_HINT,
            };
            return;
        }

        let mask = SPLIT_FANOUT - 1;
        let mut child_entries: [Vec<usize>; SPLIT_FANOUT] = std::array::from_fn(|_| Vec::new());
        let mut child_bitmap = 0_u64;
        let mut duplicate_value = ids.first().map(|&id| data[id]);
        for &id in ids {
            if duplicate_value.is_some_and(|value| data[id] != value) {
                duplicate_value = None;
            }
            let child_idx = ((data[id] >> bit_offset) as usize) & mask;
            child_entries[child_idx].push(id);
            child_bitmap |= 1_u64 << child_idx;
        }

        let child_count = child_bitmap.count_ones() as usize;
        if child_count <= 1 {
            let mut entries = ids.to_vec();
            entries.sort_unstable_by_key(|&id| data[id]);
            nodes[node_idx] = FlatNode {
                entries,
                bit_offset,
                child_bitmap: 0,
                children_start: 0,
                duplicate_hint: if duplicate_value.is_some()
                    && ids.len() >= DUPLICATE_GROUP_THRESHOLD
                {
                    0
                } else {
                    NO_DUPLICATE_HINT
                },
            };
            return;
        }

        let children_start = child_slots.len();
        child_slots.resize(children_start + SPLIT_FANOUT, u32::MAX);

        let child_nodes_start = nodes.len();
        nodes.resize_with(child_nodes_start + child_count, FlatNode::default);

        let next_bit_offset = bit_offset + SPLIT_BITS;
        let mut child_node_idx = child_nodes_start;
        for (child_pos, entries) in child_entries.into_iter().enumerate() {
            if !entries.is_empty() {
                child_slots[children_start + child_pos] =
                    u32::try_from(child_node_idx).expect("flat node index exceeds u32");
                Self::fill_node(
                    child_node_idx,
                    &entries,
                    data,
                    next_bit_offset,
                    nodes,
                    child_slots,
                );
                child_node_idx += 1;
            }
        }

        nodes[node_idx] = FlatNode {
            entries: Vec::new(),
            bit_offset,
            child_bitmap,
            children_start,
            duplicate_hint: NO_DUPLICATE_HINT,
        };
    }

    #[allow(dead_code)]
    pub(crate) fn total_len(&self) -> usize {
        self.nodes.iter().map(|node| node.entries.len()).sum()
    }

    #[allow(dead_code)]
    pub(crate) fn all_empty(&self) -> bool {
        self.nodes.iter().all(|node| node.entries.is_empty())
    }

    pub(crate) fn for_each_leaf<F: FnMut(&Vec<usize>)>(&self, f: &mut F) {
        for node in &self.nodes {
            if node.child_bitmap == 0 {
                f(&node.entries);
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn for_each_leaf_mut<F: FnMut(&mut Vec<usize>)>(&mut self, f: &mut F) {
        for node in &mut self.nodes {
            if node.child_bitmap == 0 {
                f(&mut node.entries);
            }
        }
    }

    pub(crate) fn tree_overhead_bytes(&self) -> usize {
        let nodes = self.nodes.capacity() * std::mem::size_of::<FlatNode>();
        let child_slots = self.child_slots.capacity() * std::mem::size_of::<u32>();
        let entries: usize = self
            .nodes
            .iter()
            .map(|node| node.entries.capacity() * std::mem::size_of::<usize>())
            .sum();
        nodes + child_slots + entries
    }

    #[allow(dead_code)]
    pub(crate) fn has_split(&self) -> bool {
        self.nodes
            .first()
            .is_some_and(|node| node.child_bitmap != 0)
    }

    #[allow(dead_code)]
    pub(crate) fn max_depth(&self) -> usize {
        if self.nodes.is_empty() {
            0
        } else {
            self.node_depth(0)
        }
    }

    #[allow(dead_code)]
    fn node_depth(&self, node_idx: usize) -> usize {
        let node = &self.nodes[node_idx];
        if node.child_bitmap == 0 {
            return 0;
        }

        let mut max_child_depth = 0;
        let child_range = node.children_start..node.children_start + SPLIT_FANOUT;
        for &child_idx in &self.child_slots[child_range] {
            if child_idx != u32::MAX {
                max_child_depth = max_child_depth.max(self.node_depth(child_idx as usize));
            }
        }
        1 + max_child_depth
    }
}

// ── Per-bucket statistics ────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BucketStats {
    pub bit_width: usize,
    pub count: usize,
    pub percentage: f64,
}

#[derive(Clone, Copy)]
struct LookupCacheEntry {
    value: u64,
    storage_id: usize,
}

enum LiveLookup {
    Pending(usize),
    Tree(usize),
}

// ── Bwspi (Dual-Index with Lazy Tree) ────────────────────────────────
//
//   crud_buckets — flat Vec<usize> per bit-width.
//     ALL mutations go here: O(1) push, O(1) swap_remove.
//     This is the source of truth.
//
//   trees — dense FlatBucket per bit-width.
//     Built LAZILY from crud_buckets when a lookup needs it.
//     NEVER touched during insert/remove/update.
//     Rebuilt per-width when `tree_dirty[w]` is true.
//
//   tree_dirty — one bool per bit-width.
//     Set true on any CRUD mutation. Cleared when tree is rebuilt.

pub struct Bwspi {
    pub(crate) data: Vec<u64>,

    // ── CRUD index (flat, O(1) operations) ─────────────────────
    pub(crate) crud_buckets: Vec<Vec<usize>>,
    pub(crate) bucket_positions: Vec<usize>,

    // ── Lookup index (LSB radix tree, ≤64 per leaf) ──────────────
    pub(crate) trees: Vec<FlatBucket>,
    pub(crate) tree_dirty: Vec<bool>,
    pending_inserts: Vec<Vec<usize>>,
    pending_membership: Vec<bool>,
    lookup_cache: Vec<Option<LookupCacheEntry>>,
    /// Number of tombstoned (dead) entries in each tree.
    /// When this exceeds 25% of the bucket, the tree is marked dirty
    /// for compaction on next access.
    tree_tombstones: Vec<usize>,

    pub(crate) live_len: usize,
}

impl Bwspi {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            crud_buckets: Vec::new(),
            bucket_positions: Vec::with_capacity(capacity),
            trees: Vec::new(),
            tree_dirty: Vec::new(),
            pending_inserts: Vec::new(),
            pending_membership: Vec::with_capacity(capacity),
            lookup_cache: Vec::new(),
            tree_tombstones: Vec::new(),
            live_len: 0,
        }
    }

    #[inline]
    fn ensure_width(&mut self, width: usize) {
        if width >= self.crud_buckets.len() {
            self.crud_buckets.resize_with(width + 1, Vec::new);
            self.trees.resize_with(width + 1, FlatBucket::new);
            self.tree_dirty.resize(width + 1, true);
            self.pending_inserts.resize_with(width + 1, Vec::new);
            self.lookup_cache.resize(width + 1, None);
            self.tree_tombstones.resize(width + 1, 0);
        }
    }

    #[inline]
    fn clear_lookup_cache_width(&mut self, width: usize) {
        if let Some(slot) = self.lookup_cache.get_mut(width) {
            *slot = None;
        }
    }

    #[inline]
    fn lookup_cache_get(&mut self, width: usize, target: u64) -> Option<usize> {
        let slot = self.lookup_cache.get_mut(width)?;
        let entry = (*slot)?;
        if entry.value == target
            && self.bucket_positions.get(entry.storage_id) != Some(&REMOVED_POSITION)
            && self.data.get(entry.storage_id) == Some(&target)
        {
            return Some(entry.storage_id);
        }
        *slot = None;
        None
    }

    #[inline]
    fn lookup_cache_put(&mut self, width: usize, target: u64, storage_id: usize) {
        if let Some(slot) = self.lookup_cache.get_mut(width) {
            *slot = Some(LookupCacheEntry {
                value: target,
                storage_id,
            });
        }
    }

    #[inline]
    fn add_pending_insert(&mut self, width: usize, storage_id: usize) {
        self.pending_inserts[width].push(storage_id);
        self.pending_membership[storage_id] = true;
    }

    fn clear_pending_width(&mut self, width: usize) {
        if width >= self.pending_inserts.len() {
            return;
        }
        for id in self.pending_inserts[width].drain(..) {
            if let Some(is_pending) = self.pending_membership.get_mut(id) {
                *is_pending = false;
            }
        }
    }

    #[inline]
    fn pending_len(&self, width: usize) -> usize {
        self.pending_inserts.get(width).map_or(0, Vec::len)
    }

    #[inline]
    fn pending_find_live_limited(&self, width: usize, target: u64, limit: usize) -> Option<usize> {
        let pending = self.pending_inserts.get(width)?;
        for &id in pending.iter().rev().take(limit) {
            if self.pending_membership.get(id) == Some(&true)
                && self.bucket_positions.get(id) != Some(&REMOVED_POSITION)
                && self.data.get(id) == Some(&target)
            {
                return Some(id);
            }
        }
        None
    }

    fn pending_find_live(&self, width: usize, target: u64) -> Option<usize> {
        let pending = self.pending_inserts.get(width)?;
        self.pending_find_live_limited(width, target, pending.len())
    }

    fn lookup_live(&mut self, width: usize, target: u64) -> Option<LiveLookup> {
        if let Some(id) = self.lookup_cache_get(width, target) {
            return if self.pending_membership.get(id) == Some(&true) {
                Some(LiveLookup::Pending(id))
            } else {
                Some(LiveLookup::Tree(id))
            };
        }

        let pending_len = self.pending_len(width);
        if pending_len > PENDING_MERGE_THRESHOLD {
            if let Some(id) = self.pending_find_live_limited(width, target, PENDING_RECENT_SCAN) {
                self.lookup_cache_put(width, target, id);
                return Some(LiveLookup::Pending(id));
            }
            self.rebuild_tree_width(width);
        } else if let Some(id) = self.pending_find_live(width, target) {
            self.lookup_cache_put(width, target, id);
            return Some(LiveLookup::Pending(id));
        }

        let id = self.trees[width].lsb_find_live(target, &self.data, &self.bucket_positions)?;
        self.lookup_cache_put(width, target, id);
        Some(LiveLookup::Tree(id))
    }
    fn pending_find_all_live(&self, width: usize, target: u64) -> Vec<usize> {
        let Some(pending) = self.pending_inserts.get(width) else {
            return Vec::new();
        };
        pending
            .iter()
            .copied()
            .filter(|&id| {
                self.pending_membership.get(id) == Some(&true)
                    && self.bucket_positions.get(id) != Some(&REMOVED_POSITION)
                    && self.data.get(id) == Some(&target)
            })
            .collect()
    }

    fn rebuild_tree_width(&mut self, width: usize) {
        self.trees[width] = FlatBucket::build_from(&self.crud_buckets[width], width, &self.data, 0);
        self.tree_dirty[width] = false;
        self.tree_tombstones[width] = 0;
        self.clear_pending_width(width);
        self.clear_lookup_cache_width(width);
    }
    // ── Lazy tree rebuild ────────────────────────────────────────
    //
    // Only rebuilds tree[w] when a lookup needs it and it's dirty.
    // Cost: O(k) where k = crud_buckets[w].len(). Amortized over
    // all lookups until the next mutation on that width.

    #[inline]
    fn ensure_tree_clean(&mut self, width: usize) {
        if width < self.tree_dirty.len() && self.tree_dirty[width] {
            self.rebuild_tree_width(width);
        } else if width < self.tree_tombstones.len() {
            // Lazy compaction: if tombstones exceed 25% of original tree size,
            // rebuild from clean crud_buckets.
            let live = self.crud_buckets.get(width).map_or(0, |b| b.len());
            let tombstones = self.tree_tombstones[width];
            let total = live + tombstones;
            if total > 0 && tombstones * 4 > total {
                self.rebuild_tree_width(width);
            }
        }
    }

    #[inline]
    pub(crate) fn shrink_trailing(&mut self) {
        while !self.crud_buckets.is_empty()
            && self.crud_buckets.last().is_some_and(|b| b.is_empty())
        {
            self.crud_buckets.pop();
            self.trees.pop();
            self.tree_dirty.pop();
            self.pending_inserts.pop();
            self.lookup_cache.pop();
            self.tree_tombstones.pop();
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
        self.crud_unlink_inner(index, width, false);
    }

    /// Unlink `index` from its CRUD bucket.
    /// When `keep_tree_clean` is true, the tree is NOT marked dirty
    /// (caller already handled the tree surgically).
    #[inline]
    fn crud_unlink_inner(&mut self, index: usize, width: usize, keep_tree_clean: bool) {
        let position = self.bucket_positions[index];
        debug_assert_ne!(position, REMOVED_POSITION);
        let bucket = &mut self.crud_buckets[width];
        let moved = *bucket.last().expect("live entry");
        bucket.swap_remove(position);
        if moved != index {
            self.bucket_positions[moved] = position;
        }
        self.bucket_positions[index] = REMOVED_POSITION;
        if !keep_tree_clean {
            self.tree_dirty[width] = true;
        }
    }

    // ── Live-stream CRUD ─────────────────────────────────────────

    #[inline]
    pub fn insert(&mut self, value: u64) -> usize {
        let index = self.data.len();
        self.data.push(value);
        let width = bit_width(value);
        self.ensure_width(width);
        let bucket = &mut self.crud_buckets[width];
        self.bucket_positions.push(bucket.len());
        self.pending_membership.push(false);
        bucket.push(index);

        // Clean trees stay stable. New IDs live in a small per-width delta
        // overlay so mixed insert+contains does not pay tree mutation cost.
        if !self.tree_dirty[width] {
            self.add_pending_insert(width, index);
        }

        self.live_len += 1;
        index
    }

    pub fn insert_bulk(&mut self, values: &[u64]) {
        if values.is_empty() {
            return;
        }
        let start_idx = self.data.len();
        self.data.extend_from_slice(values);
        let mut counts = [0usize; 65];
        let mut max_width = 0;
        for &value in values {
            let w = bit_width(value);
            unsafe {
                *counts.get_unchecked_mut(w) += 1;
            }
            if w > max_width {
                max_width = w;
            }
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
        self.pending_membership
            .resize(start_idx + values.len(), false);
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

        // Eager tree build: build trees now so subsequent operations
        // (contains, remove) don't pay the lazy rebuild cost.
        for (w, &count) in counts.iter().enumerate().take(max_width + 1) {
            if count > 0 {
                self.ensure_tree_clean(w);
            }
        }
    }

    #[inline]
    pub fn contains(&mut self, target: u64) -> bool {
        let w = bit_width(target);
        self.ensure_tree_clean(w);
        if w >= self.trees.len() {
            return false;
        }
        self.lookup_live(w, target).is_some()
    }

    #[inline]
    pub fn find(&mut self, target: u64) -> Option<usize> {
        let w = bit_width(target);
        self.ensure_tree_clean(w);
        if w >= self.trees.len() {
            return None;
        }
        match self.lookup_live(w, target)? {
            LiveLookup::Pending(id) | LiveLookup::Tree(id) => Some(id),
        }
    }

    #[must_use]
    pub fn find_all(&mut self, target: u64) -> Vec<usize> {
        let w = bit_width(target);
        self.ensure_tree_clean(w);
        if w >= self.trees.len() {
            return Vec::new();
        }
        let mut found = self.trees[w].lsb_find_all_live(target, &self.data, &self.bucket_positions);
        found.extend(self.pending_find_all_live(w, target));
        found
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<u64> {
        (self.bucket_positions.get(index)? != &REMOVED_POSITION).then(|| self.data[index])
    }

    pub fn remove(&mut self, target: u64) -> bool {
        let w = bit_width(target);
        if w >= self.crud_buckets.len() {
            return false;
        }

        self.ensure_tree_clean(w);
        if w >= self.trees.len() {
            return false;
        }

        match self.lookup_live(w, target) {
            Some(LiveLookup::Pending(i)) => {
                self.pending_membership[i] = false;
                self.crud_unlink_inner(i, w, true);
                self.live_len -= 1;
                self.clear_lookup_cache_width(w);
                self.shrink_trailing();
                true
            }
            Some(LiveLookup::Tree(i)) => {
                // Tombstone: just unlink from CRUD bucket.
                // The dead entry stays in the tree — lookups skip it.
                self.crud_unlink_inner(i, w, true);
                self.live_len -= 1;
                self.tree_tombstones[w] += 1;
                self.clear_lookup_cache_width(w);
                self.shrink_trailing();
                true
            }
            None => false,
        }
    }

    pub fn remove_at(&mut self, index: usize) -> bool {
        let Some(&position) = self.bucket_positions.get(index) else {
            return false;
        };
        if position == REMOVED_POSITION {
            return false;
        }

        let value = self.data[index];
        let width = bit_width(value);

        let was_pending = self.pending_membership.get(index).copied().unwrap_or(false);
        if was_pending {
            self.pending_membership[index] = false;
        }

        // Tombstone: just unlink from CRUD bucket.
        // Don't touch the tree — the dead entry is skipped by lookups.
        self.crud_unlink_inner(index, width, true);
        self.live_len -= 1;

        // Track tombstone count only for entries that are actually in the tree.
        if !was_pending
            && width < self.tree_tombstones.len()
            && !self.tree_dirty.get(width).copied().unwrap_or(true)
        {
            self.tree_tombstones[width] += 1;
        }

        self.clear_lookup_cache_width(width);
        self.shrink_trailing();
        true
    }

    pub fn update(&mut self, old: u64, new: u64) -> bool {
        self.find(old).is_some_and(|i| self.update_at(i, new))
    }

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
        let was_pending = self.pending_membership.get(index).copied().unwrap_or(false);
        self.clear_lookup_cache_width(old_width);
        if new_width != old_width {
            self.clear_lookup_cache_width(new_width);
        }

        if old_width == new_width {
            // Same width pending entries are not in the tree yet; base entries
            // still need a rebuild because their sorted leaf position changed.
            self.data[index] = new;
            if !was_pending {
                self.tree_dirty[old_width] = true;
            }
            return true;
        }

        if was_pending {
            self.pending_membership[index] = false;
        }

        // Different width — rebucket in flat index only.
        self.crud_unlink(index, old_width); // O(1)
        self.data[index] = new;
        self.crud_link(index, new_width); // O(1)
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
    pub fn sort_in_place(&mut self) -> super::SarkarSortStats {
        self.sarkar_sort()
    }

    /// Rebuild all dirty trees and merge any pending overlay IDs.
    fn rebuild_all_dirty(&mut self) {
        for w in 0..self.tree_dirty.len() {
            let has_pending = self
                .pending_inserts
                .get(w)
                .is_some_and(|pending| !pending.is_empty());
            let has_tombstones = self.tree_tombstones.get(w).copied().unwrap_or(0) > 0;
            if self.tree_dirty[w] || has_pending || has_tombstones {
                self.rebuild_tree_width(w);
            }
        }
    }

    // ── Observability ────────────────────────────────────────────

    #[inline]
    pub fn len(&self) -> usize {
        self.live_len
    }
    #[inline]
    pub fn storage_len(&self) -> usize {
        self.data.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live_len == 0
    }
    #[inline]
    pub fn data(&self) -> &[u64] {
        &self.data
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.data
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, _)| self.bucket_positions[*i] != REMOVED_POSITION)
    }

    #[inline]
    pub fn bucket_slots(&self) -> usize {
        self.crud_buckets.len()
    }

    pub fn active_bucket_count(&self) -> usize {
        self.crud_buckets.iter().filter(|b| !b.is_empty()).count()
    }

    pub fn max_bucket_size(&self) -> usize {
        self.crud_buckets.iter().map(|b| b.len()).max().unwrap_or(0)
    }

    #[inline]
    pub fn bucket_size(&self, width: usize) -> usize {
        if width < self.crud_buckets.len() {
            self.crud_buckets[width].len()
        } else {
            0
        }
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
                    percentage: if total == 0.0 {
                        0.0
                    } else {
                        count as f64 / total * 100.0
                    },
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
        let tree_outer = self.trees.capacity() * std::mem::size_of::<FlatBucket>();
        let tree_inner: usize = self.trees.iter().map(|t| t.tree_overhead_bytes()).sum();
        let pending_outer = self.pending_inserts.capacity() * std::mem::size_of::<Vec<usize>>();
        let pending_inner: usize = self.pending_inserts.iter().map(|p| p.capacity() * 8).sum();
        let pending_flags = self.pending_membership.capacity() * std::mem::size_of::<bool>();
        let cache_bytes =
            self.lookup_cache.capacity() * std::mem::size_of::<Option<LookupCacheEntry>>();
        let dirty_bytes = self.tree_dirty.capacity();
        data_bytes
            + pos_bytes
            + crud_outer
            + crud_inner
            + tree_outer
            + tree_inner
            + pending_outer
            + pending_inner
            + pending_flags
            + cache_bytes
            + dirty_bytes
    }
}

impl Default for Bwspi {
    fn default() -> Self {
        Self::new()
    }
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
