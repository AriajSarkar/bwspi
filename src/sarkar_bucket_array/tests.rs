use super::{bit_width, simd::simd_contains, Bwspi};

#[test]
fn bit_width_handles_the_full_u64_domain() {
    assert_eq!(bit_width(0), 0);
    assert_eq!(bit_width(1), 1);
    assert_eq!(bit_width(3), 2);
    assert_eq!(bit_width(4), 3);
    assert_eq!(bit_width((1_u64 << 63) - 1), 63);
    assert_eq!(bit_width(1_u64 << 63), 64);
    assert_eq!(bit_width(u64::MAX), 64);
}

#[test]
fn insert_find_and_soft_delete_keep_historical_storage() {
    let mut index = Bwspi::new();
    let first = index.insert(42);
    let duplicate = index.insert(42);
    let zero = index.insert(0);

    assert_eq!(index.len(), 3);
    assert_eq!(index.storage_len(), 3);
    assert_eq!(index.find_all(42), vec![first, duplicate]);
    assert!(index.remove_at(first));
    assert_eq!(index.len(), 2);
    assert_eq!(index.storage_len(), 3);
    assert_eq!(index.get(first), None);
    assert_eq!(index.data()[first], 42);
    assert_eq!(index.find_all(42), vec![duplicate]);
    assert!(index.contains(0));
    assert_eq!(index.get(zero), Some(0));
    assert!(!index.remove_at(first));
}

#[test]
fn update_rebuckets_in_constant_time_after_id_lookup() {
    let mut index = Bwspi::new();
    let low = index.insert(127); // width 7
    let same_width = index.insert(100); // width 7
    let high = index.insert(1_u64 << 40); // width 41

    assert!(index.update_at(low, 1_u64 << 40));
    assert!(!index.contains(127));
    assert!(index.contains(1_u64 << 40));
    assert_eq!(index.bucket_size(7), 1);
    assert_eq!(index.bucket_size(41), 2);
    assert_eq!(index.get(low), Some(1_u64 << 40));

    assert!(index.update(100, 101));
    assert_eq!(index.get(same_width), Some(101));
    assert!(index.remove_at(high));
    assert!(!index.update_at(high, 7));
    assert_eq!(index.len(), 2);
}

#[test]
fn swap_remove_repairs_the_moved_entry_position() {
    let mut index = Bwspi::new();
    let a = index.insert(64);
    let b = index.insert(65);
    let c = index.insert(66);
    assert!(index.remove_at(b));
    assert!(index.update_at(c, 7));
    assert!(index.contains(64));
    assert!(!index.contains(65));
    assert!(!index.contains(66));
    assert!(index.contains(7));
    assert_eq!(index.get(a), Some(64));
}

#[test]
fn snapshot_sort_is_ascending_and_does_not_change_storage_ids() {
    let values = [u64::MAX, 9, 0, 8, 1_u64 << 63, 9, 255, 256, 2, 1];
    let mut index = Bwspi::new();
    let ids: Vec<_> = values
        .into_iter()
        .map(|value| index.insert(value))
        .collect();
    assert!(index.remove_at(ids[3]));
    assert!(index.update_at(ids[8], 7));

    let before = index.data().to_vec();
    let mut expected: Vec<_> = index.iter().map(|(_, value)| value).collect();
    expected.sort_unstable();
    assert_eq!(index.sorted_snapshot(), expected);
    assert_eq!(index.data(), before);
    assert_eq!(index.get(ids[0]), Some(u64::MAX));
}

#[test]
fn sarkar_sort_orders_live_values_and_rebinds_the_index() {
    let mut index = Bwspi::with_capacity(128);
    let values = [
        u64::MAX,
        0,
        512,
        511,
        42,
        42,
        1_u64 << 63,
        1,
        1024,
        511,
        3,
        2,
        255,
        256,
        65,
        64,
    ];
    let ids: Vec<_> = values
        .into_iter()
        .map(|value| index.insert(value))
        .collect();
    assert!(index.remove_at(ids[4]));
    assert!(index.update_at(ids[3], 17));

    let mut expected: Vec<_> = index.iter().map(|(_, value)| value).collect();
    expected.sort_unstable();
    let stats = index.sarkar_sort();

    assert!(stats.buckets_sorted > 0);
    assert_eq!(&index.data()[..index.len()], expected);
    assert_eq!(
        index.iter().map(|(_, value)| value).collect::<Vec<_>>(),
        expected
    );
    for value in expected {
        let location = index.find(value).expect("sorted value remains indexed");
        assert_eq!(index.get(location), Some(value));
        assert!(simd_contains(&mut index, value));
    }
    assert!(index.contains(42)); // one duplicate was intentionally retained
}

#[test]
fn sarkar_sort_matches_std_sort_across_a_large_mixed_stream() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut index = Bwspi::with_capacity(4_096);
    for position in 0..4_096 {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;
        let value = match position % 7 {
            0 => state & 0xff,
            1 => (1_u64 << 31) | (state & 0xffff),
            2 => (1_u64 << 63) | (state & 0xffff),
            3 => state % 1_000,
            _ => state,
        };
        index.insert(value);
    }
    for id in (0..4_096).step_by(37) {
        assert!(index.remove_at(id));
    }

    let mut expected: Vec<_> = index.iter().map(|(_, value)| value).collect();
    expected.sort_unstable();
    index.sarkar_sort();

    assert_eq!(index.len(), expected.len());
    assert_eq!(&index.data()[..index.len()], expected);
    assert_eq!(index.sorted_snapshot(), expected);
}

#[test]
fn sarkar_sort_preserves_future_crud_correctness() {
    let mut index = Bwspi::new();
    index.insert_bulk(&[100, 3, 99, 4, 98, 5]);
    index.sarkar_sort();
    let inserted = index.insert(77);
    assert!(index.update_at(inserted, 1_u64 << 40));
    assert!(index.remove(99));
    assert!(index.contains(100));
    assert!(index.contains(1_u64 << 40));
    assert!(!index.contains(99));

    let mut expected: Vec<_> = index.iter().map(|(_, value)| value).collect();
    expected.sort_unstable();
    assert_eq!(index.sorted_snapshot(), expected);
}

#[test]
fn recursive_splitting_for_uniform_distribution() {
    let mut index = Bwspi::new();
    // Bucket 17 covers 65,536 to 131,071.
    for i in 0..200 {
        index.insert(65536 + i);
    }

    // 200 items exceeds LEAF_CAP (64), so the tree for width 17 should
    // have split into children (it's now a router, not a leaf).
    // Trigger lazy tree build with a lookup first.
    assert!(index.contains(65536 + 50));
    assert!(index.trees[17].has_split());

    assert!(index.contains(65536 + 50));
    assert!(index.contains(65536 + 150));
    assert!(!index.contains(65536 + 250)); // Missing value

    // Check finding
    assert!(index.find(65536 + 50).is_some());
    assert!(index.find(100000).is_none());

    // Check sorting
    let mut expected: Vec<_> = index.iter().map(|(_, value)| value).collect();
    expected.sort_unstable();
    index.sarkar_sort();
    assert_eq!(&index.data()[..index.len()], expected);
}

#[test]
fn deep_recursive_split_stress_test() {
    // Insert 10,000 elements in one bit-width bucket to trigger
    // multiple levels of recursive splitting.
    let mut index = Bwspi::new();
    // All values in bit-width 32: range [2^31, 2^32 - 1]
    for i in 0..10_000_u64 {
        index.insert((1_u64 << 31) | i);
    }

    assert_eq!(index.len(), 10_000);
    assert_eq!(index.bucket_size(32), 10_000);
    // Trigger lazy tree build before checking structure.
    assert!(index.contains((1_u64 << 31) | 0));
    // The tree must have split recursively.
    assert!(index.trees[32].has_split());
    // Max depth should be > 1 (sub-of-sub splitting).
    assert!(index.trees[32].max_depth() > 1,
        "expected recursive splits, got depth {}",
        index.trees[32].max_depth());

    // Every value must be findable.
    for i in 0..10_000_u64 {
        let v = (1_u64 << 31) | i;
        assert!(index.contains(v), "missing {v}");
    }

    // Remove half, verify the other half.
    for i in (0..10_000_u64).step_by(2) {
        assert!(index.remove((1_u64 << 31) | i));
    }
    assert_eq!(index.len(), 5_000);
    for i in (1..10_000_u64).step_by(2) {
        assert!(index.contains((1_u64 << 31) | i));
    }

    // Sorting still works.
    let mut expected: Vec<_> = index.iter().map(|(_, value)| value).collect();
    expected.sort_unstable();
    index.sarkar_sort();
    assert_eq!(&index.data()[..index.len()], expected);
}
