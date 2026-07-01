use super::index::Bwspi;

/// Independent bucket-aware snapshot sorting.
///
/// Bit width is monotonic for unsigned values: every value in bucket `b` is
/// smaller than every value in bucket `b + 1`. Within each bucket, all values
/// are collected and sorted together (since LSB radix tree leaf traversal order
/// does NOT correspond to value order).
pub(crate) fn sorted_snapshot(index: &Bwspi) -> Vec<u64> {
    let mut output = Vec::with_capacity(index.len());

    for width in 0..index.trees.len() {
        let start = output.len();
        index.trees[width].for_each_leaf(&mut |leaf| {
            output.extend(leaf.iter().map(|&id| index.data[id]));
        });
        // Sort the entire width bucket (not just individual leaves)
        output[start..].sort_unstable();
    }

    output
}
