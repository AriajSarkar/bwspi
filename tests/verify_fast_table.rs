//! Standalone verification of the fast_table accelerator.
//!
//! Run with:  cargo test --test verify_fast_table
//!
//! This test independently verifies that the fast_table-based contains/find/find_all
//! produce IDENTICAL results to a brute-force linear scan of the data array.

use bwspi::Bwspi;

/// Brute-force reference: scan ALL data to check contains.
fn brute_contains(bwspi: &Bwspi, target: u64) -> bool {
    bwspi.iter().any(|(_, v)| v == target)
}

/// Brute-force reference: scan ALL data to find first occurrence.
fn brute_find(bwspi: &Bwspi, target: u64) -> Option<usize> {
    bwspi.iter().find(|(_, v)| *v == target).map(|(id, _)| id)
}

/// Brute-force reference: scan ALL data to find ALL occurrences.
fn brute_find_all(bwspi: &Bwspi, target: u64) -> Vec<usize> {
    bwspi.iter().filter(|(_, v)| *v == target).map(|(id, _)| id).collect()
}

struct XorShift64(u64);
impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut v = self.0;
        v ^= v << 13;
        v ^= v >> 7;
        v ^= v << 17;
        self.0 = v;
        v
    }
}

#[test]
fn verify_contains_matches_brute_force_high_entropy() {
    let mut rng = XorShift64(0xdead_beef_cafe_babe);
    let n = 50_000;
    let mut bwspi = Bwspi::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    for _ in 0..n {
        let bits = (rng.next() % 65) as u32;
        let val = if bits == 0 { 0 } else if bits == 64 { rng.next() } else {
            let min = 1u64 << (bits - 1);
            let range = 1u64 << bits;
            min + rng.next() % (range - min)
        };
        bwspi.insert(val);
        values.push(val);
    }

    // Check every inserted value
    for &v in &values {
        let fast = bwspi.contains(v);
        let brute = brute_contains(&bwspi, v);
        assert_eq!(fast, brute, "MISMATCH for value {}: fast_table={}, brute={}", v, fast, brute);
    }

    // Check many random non-existent values
    for _ in 0..10_000 {
        let probe = rng.next();
        let fast = bwspi.contains(probe);
        let brute = brute_contains(&bwspi, probe);
        assert_eq!(fast, brute, "MISMATCH for probe {}: fast_table={}, brute={}", probe, fast, brute);
    }
}

#[test]
fn verify_find_matches_brute_force() {
    let mut rng = XorShift64(0x1234_5678_9abc_def0);
    let n = 20_000;
    let mut bwspi = Bwspi::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    for _ in 0..n {
        let val = rng.next();
        bwspi.insert(val);
        values.push(val);
    }

    for &v in &values {
        let fast = bwspi.find(v);
        let brute = brute_find(&bwspi, v);
        // Both should be Some, and the found ID should map to the correct value
        assert!(fast.is_some(), "fast_table find returned None for {}", v);
        assert!(brute.is_some(), "brute find returned None for {}", v);
        assert_eq!(bwspi.get(fast.unwrap()), Some(v));
    }
}

#[test]
fn verify_find_all_matches_brute_force_with_duplicates() {
    let mut bwspi = Bwspi::new();
    // Insert many duplicates
    for _ in 0..100 { bwspi.insert(42); }
    for _ in 0..50 { bwspi.insert(100); }
    for _ in 0..200 { bwspi.insert(7); }
    for _ in 0..1 { bwspi.insert(0); }
    for _ in 0..75 { bwspi.insert(u64::MAX); }

    for target in [42u64, 100, 7, 0, u64::MAX, 999] {
        let mut fast = bwspi.find_all(target);
        let mut brute = brute_find_all(&bwspi, target);
        fast.sort();
        brute.sort();
        assert_eq!(fast, brute, "find_all MISMATCH for {}: fast={:?} brute={:?}", target, fast.len(), brute.len());
    }
}

#[test]
fn verify_after_removes_and_updates() {
    let mut rng = XorShift64(0xaaaa_bbbb_cccc_dddd);
    let mut bwspi = Bwspi::new();
    let mut reference: Vec<Option<u64>> = Vec::new();

    // Insert 5000 values
    for _ in 0..5000 {
        let val = rng.next();
        let id = bwspi.insert(val);
        assert_eq!(id, reference.len());
        reference.push(Some(val));
    }

    // Remove 1000 random ones
    for _ in 0..1000 {
        let id = (rng.next() % reference.len() as u64) as usize;
        if reference[id].is_some() {
            assert!(bwspi.remove_at(id));
            reference[id] = None;
        }
    }

    // Update 500 random ones
    for _ in 0..500 {
        let id = (rng.next() % reference.len() as u64) as usize;
        if reference[id].is_some() {
            let new_val = rng.next();
            assert!(bwspi.update_at(id, new_val));
            reference[id] = Some(new_val);
        }
    }

    // Now verify every single live value
    let live: Vec<(usize, u64)> = reference.iter().enumerate()
        .filter_map(|(i, v)| v.map(|val| (i, val)))
        .collect();

    assert_eq!(bwspi.len(), live.len(), "live count mismatch");

    for &(id, val) in &live {
        assert_eq!(bwspi.get(id), Some(val), "get mismatch at id {}", id);
        assert!(bwspi.contains(val), "contains returned false for live value {}", val);
    }

    // Check 5000 random probes
    for _ in 0..5000 {
        let probe = rng.next();
        let fast = bwspi.contains(probe);
        let brute = brute_contains(&bwspi, probe);
        assert_eq!(fast, brute, "post-mutation MISMATCH for probe {}", probe);
    }
}

#[test]
fn verify_uniform_width_worst_case() {
    // ALL values in the same bit-width bucket — the worst case for BWSPI
    // This forces a single massive fast_table
    let mut bwspi = Bwspi::new();
    let base = 1u64 << 31;
    for i in 0..10_000u64 {
        bwspi.insert(base + i);
    }

    // Every value must be findable
    for i in 0..10_000u64 {
        assert!(bwspi.contains(base + i), "missing {} in uniform-width bucket", base + i);
    }

    // Values outside the range must NOT be found
    for i in 10_000..10_100u64 {
        assert!(!bwspi.contains(base + i), "false positive for {} in uniform-width bucket", base + i);
    }
}

#[test]
fn verify_sorted_snapshot_still_correct_after_fast_table() {
    let mut rng = XorShift64(0xfeed_face_1234_5678);
    let mut bwspi = Bwspi::new();
    for _ in 0..5000 {
        bwspi.insert(rng.next());
    }
    // Remove some
    for id in (0..5000).step_by(3) {
        bwspi.remove_at(id);
    }

    let snapshot = bwspi.sorted_snapshot();
    let mut expected: Vec<u64> = bwspi.iter().map(|(_, v)| v).collect();
    expected.sort_unstable();
    assert_eq!(snapshot, expected, "sorted_snapshot doesn't match std sort");
}

#[test]
fn verify_sarkar_sort_still_correct_after_fast_table() {
    let mut rng = XorShift64(0x0bad_cafe_dead_c0de);
    let mut bwspi = Bwspi::new();
    for _ in 0..5000 {
        bwspi.insert(rng.next());
    }
    for id in (0..5000).step_by(5) {
        bwspi.remove_at(id);
    }

    let mut expected: Vec<u64> = bwspi.iter().map(|(_, v)| v).collect();
    expected.sort_unstable();
    bwspi.sarkar_sort();
    assert_eq!(&bwspi.data()[..bwspi.len()], &expected[..]);

    // After sort, CRUD ops must still work
    bwspi.insert(42);
    assert!(bwspi.contains(42));
    bwspi.remove(42);
    assert!(!bwspi.contains(42));
}

#[test]
fn verify_insert_bulk_matches_individual_insert() {
    let mut rng = XorShift64(0x5555_6666_7777_8888);
    let values: Vec<u64> = (0..10_000).map(|_| rng.next()).collect();

    // Method 1: insert_bulk
    let mut bulk = Bwspi::with_capacity(values.len());
    bulk.insert_bulk(&values);

    // Method 2: individual insert
    let mut individual = Bwspi::with_capacity(values.len());
    for &v in &values {
        individual.insert(v);
    }

    // Both should contain the exact same data
    assert_eq!(bulk.len(), individual.len());
    assert_eq!(bulk.data(), individual.data());

    // Both should answer identical queries
    for &v in &values {
        assert_eq!(bulk.contains(v), individual.contains(v));
    }
}
