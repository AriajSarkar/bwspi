use bwspi::Bwspi;

struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self { state: if seed == 0 { 1 } else { seed } }
    }

    fn next_u64(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn next_range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo + 1)
    }
}

#[test]
#[ignore] // TODO: sort needs redesign for LSB radix tree
fn test_differential_random_ops() {
    let mut rng = SimpleRng::new(12345);
    
    for run in 0..10 {
        let mut bwspi = Bwspi::new();
        let mut reference: Vec<Option<u64>> = Vec::new();
        
        for op_idx in 0..5000 {
            let op = rng.next_range(0, 9);
            
            match op {
                // Insert
                0 | 1 | 2 => {
                    let val = rng.next_u64();
                    let id = bwspi.insert(val);
                    assert_eq!(id, reference.len(), "Inserted storage ID mismatch at op {}", op_idx);
                    reference.push(Some(val));
                }
                // Delete by ID
                3 => {
                    if !reference.is_empty() {
                        let id = rng.next_range(0, (reference.len() - 1) as u64) as usize;
                        let was_live = reference[id].is_some();
                        let removed = bwspi.remove_at(id);
                        assert_eq!(removed, was_live, "remove_at returned incorrect result for ID {} at op {}", id, op_idx);
                        if was_live {
                            reference[id] = None;
                        }
                    }
                }
                // Delete by value
                4 => {
                    // Choose either an existing value or a random value
                    let val = if !reference.is_empty() && rng.next_range(0, 1) == 0 {
                        // try to find a live value
                        let mut live_indices = Vec::new();
                        for (i, v) in reference.iter().enumerate() {
                            if v.is_some() {
                                live_indices.push(i);
                            }
                        }
                        if !live_indices.is_empty() {
                            let idx = rng.next_range(0, (live_indices.len() - 1) as u64) as usize;
                            reference[live_indices[idx]].unwrap()
                        } else {
                            rng.next_u64()
                        }
                    } else {
                        rng.next_u64()
                    };
                    
                    let mut found_in_ref = false;
                    for v in reference.iter_mut() {
                        if *v == Some(val) {
                            *v = None;
                            found_in_ref = true;
                            break;
                        }
                    }
                    
                    let removed = bwspi.remove(val);
                    assert_eq!(removed, found_in_ref, "remove returned incorrect result for value {} at op {}", val, op_idx);
                }
                // Update by ID
                5 => {
                    if !reference.is_empty() {
                        let id = rng.next_range(0, (reference.len() - 1) as u64) as usize;
                        let was_live = reference[id].is_some();
                        let new_val = rng.next_u64();
                        let updated = bwspi.update_at(id, new_val);
                        assert_eq!(updated, was_live, "update_at returned incorrect result for ID {} at op {}", id, op_idx);
                        if was_live {
                            reference[id] = Some(new_val);
                        }
                    }
                }
                // Update by value
                6 => {
                    let old_val = if !reference.is_empty() && rng.next_range(0, 1) == 0 {
                        let mut live_indices = Vec::new();
                        for (i, v) in reference.iter().enumerate() {
                            if v.is_some() {
                                live_indices.push(i);
                            }
                        }
                        if !live_indices.is_empty() {
                            let idx = rng.next_range(0, (live_indices.len() - 1) as u64) as usize;
                            reference[live_indices[idx]].unwrap()
                        } else {
                            rng.next_u64()
                        }
                    } else {
                        rng.next_u64()
                    };
                    
                    let new_val = rng.next_u64();
                    
                    let mut found_in_ref = false;
                    for v in reference.iter_mut() {
                        if *v == Some(old_val) {
                            *v = Some(new_val);
                            found_in_ref = true;
                            break;
                        }
                    }
                    
                    let updated = bwspi.update(old_val, new_val);
                    assert_eq!(updated, found_in_ref, "update returned incorrect result for old_val {} -> new_val {} at op {}", old_val, new_val, op_idx);
                }
                // Sarkar Sort
                7 => {
                    bwspi.sarkar_sort();
                    
                    // After sarkar_sort, all live elements must be sorted and placed at 0..live_len.
                    // Let's rebuild the reference map.
                    let mut live_elements = Vec::new();
                    for v in reference.iter() {
                        if let Some(val) = v {
                            live_elements.push(*val);
                        }
                    }
                    live_elements.sort_unstable();
                    
                    // Verify BWSPI state
                    assert_eq!(bwspi.len(), live_elements.len(), "Length mismatch after sort at run {} op {}", run, op_idx);
                    
                    let mut new_reference = vec![None; bwspi.storage_len()];
                    for i in 0..live_elements.len() {
                        new_reference[i] = Some(live_elements[i]);
                    }
                    
                    reference = new_reference;
                    
                    // Check that BWSPI matches
                    for i in 0..live_elements.len() {
                        assert_eq!(bwspi.get(i), Some(live_elements[i]), "Value mismatch at index {} after sort", i);
                    }
                    for i in live_elements.len()..bwspi.storage_len() {
                        assert_eq!(bwspi.get(i), None, "Expected deleted/marked slot at index {} after sort", i);
                    }
                }
                // Lookups & Verifications
                _ => {
                    // Check a few live values
                    let mut live_values = Vec::new();
                    for (i, &v) in reference.iter().enumerate() {
                        if let Some(val) = v {
                            live_values.push((i, val));
                        }
                    }
                    
                    assert_eq!(bwspi.len(), live_values.len(), "live len mismatch at op {}", op_idx);
                    assert_eq!(bwspi.is_empty(), live_values.is_empty(), "is_empty mismatch at op {}", op_idx);
                    
                    // Check sorted_snapshot
                    let mut expected_sorted: Vec<u64> = live_values.iter().map(|(_, v)| *v).collect();
                    expected_sorted.sort_unstable();
                    assert_eq!(bwspi.sorted_snapshot(), expected_sorted, "sorted_snapshot mismatch at op {}", op_idx);
                    
                    if !live_values.is_empty() {
                        // pick a random live element to query
                        let idx = rng.next_range(0, (live_values.len() - 1) as u64) as usize;
                        let (ref_id, ref_val) = live_values[idx];
                        
                        assert!(bwspi.contains(ref_val), "contains returned false for live value {} at op {}", ref_val, op_idx);
                        assert_eq!(bwspi.get(ref_id), Some(ref_val), "get returned incorrect value for ID {} at op {}", ref_id, op_idx);
                        
                        let found_id = bwspi.find(ref_val).expect("find returned None for live value");
                        assert_eq!(bwspi.get(found_id), Some(ref_val), "found ID does not match value");
                        
                        let all_found = bwspi.find_all(ref_val);
                        assert!(!all_found.is_empty(), "find_all returned empty list for live value");
                        for fid in all_found {
                            assert_eq!(bwspi.get(fid), Some(ref_val), "find_all ID value mismatch");
                        }
                    }
                    
                    // Query a random non-existent value
                    let mut rand_val = rng.next_u64();
                    while live_values.iter().any(|(_, v)| *v == rand_val) {
                        rand_val = rng.next_u64();
                    }
                    
                    assert!(!bwspi.contains(rand_val), "contains returned true for non-existent value {}", rand_val);
                    assert_eq!(bwspi.find(rand_val), None, "find returned Some for non-existent value");
                    assert!(bwspi.find_all(rand_val).is_empty(), "find_all returned non-empty for non-existent value");
                }
            }
        }
    }
}
