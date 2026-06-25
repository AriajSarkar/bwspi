use super::index::{bit_width, Bwspi};

/// Sarkar Sort statistics returned by [`Bwspi::sarkar_sort`].
#[derive(Debug, Clone, Default)]
pub struct SarkarSortStats {
    pub buckets_sorted: usize,
    pub swaps: usize,
}

/// Sort using tree leaves for ordering, cycle permutation for data
/// movement, then rebuild both indexes from the sorted result.
pub(crate) fn sort_in_place(index: &mut Bwspi) -> SarkarSortStats {
    let mut stats = SarkarSortStats::default();

    // Phase A: sort each tree leaf's entries by data value.
    for width in 0..index.trees.len() {
        index.trees[width].for_each_leaf_mut(&mut |leaf| {
            if leaf.len() > 1 {
                stats.buckets_sorted += 1;
                leaf.sort_unstable_by_key(|&id| index.data[id]);
            } else if !leaf.is_empty() {
                stats.buckets_sorted += 1;
            }
        });
    }

    // Phase B: walk all tree leaves in order to assign destinations.
    let mut destination = 0usize;
    for width in 0..index.trees.len() {
        index.trees[width].for_each_leaf(&mut |leaf| {
            for &storage_id in leaf.iter() {
                index.bucket_positions[storage_id] = destination;
                destination += 1;
            }
        });
    }
    debug_assert_eq!(destination, index.live_len);

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

    // Phase D: rebind BOTH indexes from the sorted data.
    // Tree: rebind leaf entries to sequential data positions.
    let mut start = 0usize;
    for width in 0..index.trees.len() {
        index.trees[width].for_each_leaf_mut(&mut |leaf| {
            for (offset, storage_id) in leaf.iter_mut().enumerate() {
                *storage_id = start + offset;
            }
            start += leaf.len();
        });
    }
    debug_assert_eq!(start, index.live_len);

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

    stats
}
