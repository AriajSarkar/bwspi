use bwspi::Bwspi;

#[test]
fn test_update_at_sub_bucket_bug() {
    let mut index = Bwspi::new();
    
    // Insert 65 copies of 100 to force a sub-bucket split
    let mut ids = Vec::new();
    for _ in 0..65 {
        ids.push(index.insert(100)); // width=7, sub_index=4
    }
    
    // Update the last one to 120 (also width=7, but sub_index=7)
    let target_id = ids.last().unwrap();
    let updated = index.update_at(*target_id, 120);
    
    println!("Updated? {}", updated);
    
    assert!(index.contains(120), "FATAL BUG: BWSPI could not find 120 because it was left in the wrong sub-bucket!");
}
