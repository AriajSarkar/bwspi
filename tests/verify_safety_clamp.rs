use bwspi::Bwspi;

#[test]
fn test_safety_clamp_extreme_load_factor() {
    // EXPERIMENT 1: Ridiculously high load factor
    // If the fast_table is 100% full (no empty sentinel slots), an open-addressing
    // hash table search will infinite loop when looking for a missing element.
    // The load_factor = 100.0 asks for exactly this. Our min_cap = N + 1 protects against it.
    
    let mut array = Bwspi::new().with_load_factor(100.0);
    
    // Insert 100 elements (must be > 64 to trigger fast_table)
    // We use numbers in the same bit-width (e.g., 256 to 355) so they fall in the same bucket.
    for i in 256..356 {
        array.insert(i);
    }
    
    // Verify it doesn't infinite loop on hits
    for i in 256..356 {
        assert!(array.contains(i));
    }
    
    // Verify it doesn't infinite loop on misses!
    // (If min_cap failed and the table is 100% full, this line would hang your PC forever)
    assert!(!array.contains(9999));
    
    // EXPERIMENT 2: Ridiculously small load factor
    // If a user passes 0.0000001, it would try to allocate terabytes of RAM and crash (OOM).
    // Our clamp limits it to 0.01 minimum.
    
    let mut array2 = Bwspi::new().with_load_factor(0.0000001);
    
    for i in 256..356 {
        array2.insert(i);
    }
    
    // Verify it works and didn't crash your RAM
    assert!(array2.contains(300));
}
