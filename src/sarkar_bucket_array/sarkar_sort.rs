use super::{bit_width, Bwspi};

const INSERTION_SORT_CUTOFF: usize = 24;
const MIDPOINT_PARTITION_BUDGET: usize = 32;

/// Work performed by [`Bwspi::sarkar_sort`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SarkarSortStats {
    /// Number of non-empty BWSPI buckets used as sort regions.
    pub buckets_sorted: usize,
    /// Number of value-range midpoint partitions.
    pub midpoint_partitions: usize,
    /// Number of candidate comparisons while locating a real value nearest a
    /// midpoint. This is the transcript-defined pivot-navigation work.
    pub pivot_probes: usize,
    /// Number of in-place swaps of bucket pointers plus storage swaps.
    pub swaps: usize,
    /// Number of regions protected by the bounded MSD-radix fallback.
    pub radix_fallbacks: usize,
}

/// Implements the transcript's "min / midpoint / closest actual value" loop
/// over existing BWSPI pointer buckets, then applies the resulting permutation
/// to the one internal data array.
pub(crate) fn sort_in_place(index: &mut Bwspi) -> SarkarSortStats {
    let mut stats = SarkarSortStats::default();

    // Phase A: BWSPI already establishes globally ordered bit-width regions.
    // Sort only their pointer arrays. No second data array is created.
    for width in 0..index.buckets.len() {
        if let Some(subs) = &mut index.sub_buckets[width] {
            for sub in subs {
                if sub.len() > 1 {
                    stats.buckets_sorted += 1;
                    sort_bucket_by_midpoint(&index.data, sub, &mut stats);
                } else if !sub.is_empty() {
                    stats.buckets_sorted += 1;
                }
            }
        } else {
            let bucket = &mut index.buckets[width];
            if bucket.len() > 1 {
                stats.buckets_sorted += 1;
                sort_bucket_by_midpoint(&index.data, bucket, &mut stats);
            } else if !bucket.is_empty() {
                stats.buckets_sorted += 1;
            }
        }
    }

    // Phase B: the sorted bucket pointers describe the desired global order.
    // Reuse the existing position map as old-location -> new-location mapping.
    let mut destination = 0usize;
    for width in 0..index.buckets.len() {
        if let Some(subs) = &index.sub_buckets[width] {
            for sub in subs {
                for &source in sub {
                    index.bucket_positions[source] = destination;
                    destination += 1;
                }
            }
        } else {
            for &source in &index.buckets[width] {
                index.bucket_positions[source] = destination;
                destination += 1;
            }
        }
    }
    debug_assert_eq!(destination, index.live_len);

    // Deleted storage remains present by soft-delete contract. Move it after
    // the live sorted prefix without allocating a tombstone side array.
    for source in 0..index.data.len() {
        if index.bucket_positions[source] == usize::MAX {
            index.bucket_positions[source] = destination;
            destination += 1;
        }
    }

    // Apply the permutation by cycles. Swapping map entries keeps the current
    // element's destination attached to it, so every element settles in-place.
    for position in 0..index.data.len() {
        while index.bucket_positions[position] != position {
            let destination = index.bucket_positions[position];
            index.data.swap(position, destination);
            index.bucket_positions.swap(position, destination);
            stats.swaps += 1;
        }
    }

    // Phase C: rebind the *existing* bucket pointer slots to their new data
    // locations. Buckets have the same lengths/capacities; only values change.
    let mut start = 0usize;
    for width in 0..index.buckets.len() {
        if let Some(subs) = &mut index.sub_buckets[width] {
            for sub in subs {
                for (offset, storage_id) in sub.iter_mut().enumerate() {
                    let location = start + offset;
                    debug_assert_eq!(bit_width(index.data[location]), width);
                    *storage_id = location;
                    index.bucket_positions[location] = offset;
                }
                start += sub.len();
            }
        } else {
            let bucket = &mut index.buckets[width];
            for (offset, storage_id) in bucket.iter_mut().enumerate() {
                let location = start + offset;
                debug_assert_eq!(bit_width(index.data[location]), width);
                *storage_id = location;
                index.bucket_positions[location] = offset;
            }
            start += bucket.len();
        }
    }
    debug_assert_eq!(start, index.live_len);
    for position in start..index.data.len() {
        index.bucket_positions[position] = usize::MAX;
    }
    index.shrink_trailing();

    stats
}

fn sort_bucket_by_midpoint(data: &[u64], indices: &mut [usize], stats: &mut SarkarSortStats) {
    midpoint_sort(data, indices, MIDPOINT_PARTITION_BUDGET, stats);
}

/// Three-way quicksort using an actual value closest to the arithmetic range
/// midpoint. Equal values form a completed central region in one partition.
fn midpoint_sort(
    data: &[u64],
    mut indices: &mut [usize],
    mut budget: usize,
    stats: &mut SarkarSortStats,
) {
    while indices.len() > INSERTION_SORT_CUTOFF {
        if budget == 0 {
            // A range midpoint can be a poor value-distribution pivot. A
            // bounded, in-place binary MSD fallback gives a fixed 64-bit
            // upper bound instead of allowing adversarial quadratic work.
            stats.radix_fallbacks += 1;
            msd_radix_sort(data, indices, 63, stats);
            return;
        }
        budget -= 1;

        let (minimum, maximum) = min_max(data, indices);
        if minimum == maximum {
            return;
        }
        let midpoint = minimum + (maximum - minimum) / 2;
        let pivot = closest_value(data, indices, midpoint, stats);
        let (lower_end, upper_start) = partition_three_way(data, indices, pivot, stats);
        stats.midpoint_partitions += 1;

        // Recurse into the smaller side and loop over the larger one. That
        // limits stack growth even when the value distribution is skewed.
        let (lower, equal_and_upper) = indices.split_at_mut(lower_end);
        let (_, upper) = equal_and_upper.split_at_mut(upper_start - lower_end);
        if lower.len() < upper.len() {
            midpoint_sort(data, lower, budget, stats);
            indices = upper;
        } else {
            midpoint_sort(data, upper, budget, stats);
            indices = lower;
        }
    }
    insertion_sort(data, indices, stats);
}

#[inline]
fn min_max(data: &[u64], indices: &[usize]) -> (u64, u64) {
    let first = data[indices[0]];
    indices
        .iter()
        .skip(1)
        .fold((first, first), |(min, max), &id| {
            let value = data[id];
            (min.min(value), max.max(value))
        })
}

#[inline]
fn closest_value(
    data: &[u64],
    indices: &[usize],
    midpoint: u64,
    stats: &mut SarkarSortStats,
) -> u64 {
    let mut candidate = data[indices[0]];
    let mut distance = candidate.abs_diff(midpoint);
    for &id in indices.iter().skip(1) {
        let value = data[id];
        let next_distance = value.abs_diff(midpoint);
        stats.pivot_probes += 1;
        // Ties choose the lower value, matching floor/ceil midpoint intent.
        if next_distance < distance || (next_distance == distance && value < candidate) {
            candidate = value;
            distance = next_distance;
        }
    }
    candidate
}

fn partition_three_way(
    data: &[u64],
    indices: &mut [usize],
    pivot: u64,
    stats: &mut SarkarSortStats,
) -> (usize, usize) {
    let mut lower = 0usize;
    let mut scan = 0usize;
    let mut upper = indices.len();

    while scan < upper {
        match data[indices[scan]].cmp(&pivot) {
            std::cmp::Ordering::Less => {
                if lower != scan {
                    indices.swap(lower, scan);
                    stats.swaps += 1;
                }
                lower += 1;
                scan += 1;
            }
            std::cmp::Ordering::Equal => scan += 1,
            std::cmp::Ordering::Greater => {
                upper -= 1;
                if scan != upper {
                    indices.swap(scan, upper);
                    stats.swaps += 1;
                }
            }
        }
    }
    (lower, upper)
}

fn insertion_sort(data: &[u64], indices: &mut [usize], stats: &mut SarkarSortStats) {
    for current in 1..indices.len() {
        let mut position = current;
        while position > 0 && data[indices[position - 1]] > data[indices[position]] {
            indices.swap(position - 1, position);
            stats.swaps += 1;
            position -= 1;
        }
    }
}

/// Allocation-free binary MSD radix sort for the rare bounded-pivot fallback.
fn msd_radix_sort(data: &[u64], indices: &mut [usize], bit: i32, stats: &mut SarkarSortStats) {
    if indices.len() < 2 || bit < 0 {
        return;
    }

    let mask = 1_u64 << bit;
    let mut lower = 0usize;
    let mut upper = indices.len();
    while lower < upper {
        while lower < upper && data[indices[lower]] & mask == 0 {
            lower += 1;
        }
        while lower < upper && data[indices[upper - 1]] & mask != 0 {
            upper -= 1;
        }
        if lower < upper {
            indices.swap(lower, upper - 1);
            stats.swaps += 1;
            lower += 1;
            upper -= 1;
        }
    }

    let (zeros, ones) = indices.split_at_mut(lower);
    msd_radix_sort(data, zeros, bit - 1, stats);
    msd_radix_sort(data, ones, bit - 1, stats);
}
