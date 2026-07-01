use super::index::{bit_width, Bwspi};

/// Sarkar Sort statistics returned by [`Bwspi::sarkar_sort`].
#[derive(Debug, Clone, Default)]
pub struct SarkarSortStats {
    pub buckets_sorted: usize,
    pub swaps: usize,
}

/// Sort using globally-sorted storage IDs, cycle permutation for data
/// movement, then rebuild both indexes from the sorted result.
pub(crate) fn sort_in_place(index: &mut Bwspi) -> SarkarSortStats {
    let mut stats = SarkarSortStats::default();

    // Phase A: collect ALL live storage IDs from all tree leaves,
    // then sort globally by data[id].
    //
    // With LSB radix trees, leaf traversal order does NOT equal value order
    // (children are ordered by trailing bits, not by value). So we collect
    // all IDs and sort them globally.
    let mut all_ids: Vec<usize> = Vec::with_capacity(index.live_len);
    for width in 0..index.trees.len() {
        index.trees[width].for_each_leaf(&mut |leaf| {
            if !leaf.is_empty() {
                stats.buckets_sorted += 1;
            }
            all_ids.extend_from_slice(leaf);
        });
    }
    all_ids.sort_unstable_by_key(|&id| index.data[id]);

    // Phase B: assign destinations in sorted order.
    for (destination, &storage_id) in all_ids.iter().enumerate() {
        index.bucket_positions[storage_id] = destination;
    }
    let mut destination = index.live_len;

    // Dead storage after the live prefix.
    for source in 0..index.data.len() {
        if index.bucket_positions[source] == usize::MAX {
            index.bucket_positions[source] = destination;
            destination += 1;
        }
    }

    // Phase C: in-place cycle permutation (zero allocation).
    for position in 0..index.data.len() {
        while index.bucket_positions[position] != position {
            let dest = index.bucket_positions[position];
            index.data.swap(position, dest);
            index.bucket_positions.swap(position, dest);
            stats.swaps += 1;
        }
    }

    // Phase D: rebuild BOTH indexes from the sorted data.
    // Flat CRUD index: rebuild from scratch — each width gets sequential IDs.
    for bucket in index.crud_buckets.iter_mut() {
        bucket.clear();
    }
    for location in 0..index.live_len {
        let width = bit_width(index.data[location]);
        if width >= index.crud_buckets.len() {
            index.crud_buckets.resize_with(width + 1, Vec::new);
        }
        index.bucket_positions[location] = index.crud_buckets[width].len();
        index.crud_buckets[width].push(location);
    }
    for position in index.live_len..index.data.len() {
        index.bucket_positions[position] = usize::MAX;
    }
    index.shrink_trailing();
    // Mark all trees dirty so they rebuild with new storage positions.
    for dirty in index.tree_dirty.iter_mut() {
        *dirty = true;
    }

    stats
}
