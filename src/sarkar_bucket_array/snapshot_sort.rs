use super::Bwspi;

/// Independent bucket-aware snapshot sorting.
///
/// Bit width is monotonic for unsigned values: every value in bucket `b` is
/// smaller than every value in bucket `b + 1`. The output can therefore be
/// sorted by sorting only the contiguous segment appended for each bucket.
pub(crate) fn sorted_snapshot(index: &Bwspi) -> Vec<u64> {
    let mut output = Vec::with_capacity(index.len());

    for bucket in &index.buckets {
        let start = output.len();
        output.extend(bucket.iter().map(|&storage_id| index.data[storage_id]));
        output[start..].sort_unstable();
    }

    output
}
