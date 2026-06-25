use super::index::Bwspi;

/// Independent bucket-aware snapshot sorting.
///
/// Bit width is monotonic for unsigned values: every value in bucket `b` is
/// smaller than every value in bucket `b + 1`. The output can therefore be
/// sorted by sorting only the contiguous segment appended for each leaf.
pub(crate) fn sorted_snapshot(index: &Bwspi) -> Vec<u64> {
    let mut output = Vec::with_capacity(index.len());

    for width in 0..index.trees.len() {
        index.trees[width].for_each_leaf(&mut |leaf| {
            let start = output.len();
            output.extend(leaf.iter().map(|&id| index.data[id]));
            output[start..].sort_unstable();
        });
    }

    output
}
