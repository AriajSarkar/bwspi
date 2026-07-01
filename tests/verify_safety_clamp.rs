use bwspi::Bwspi;

#[test]
fn test_lsb_radix_large_bucket_correctness() {
    // Verify the LSB radix tree handles large buckets correctly.
    // With no hash table and no empty slots, we must verify that
    // the bit-extraction routing finds all values without collision.

    let mut array = Bwspi::new();

    // Insert 200 elements in the same bit-width bucket (9-bit: 256..455)
    // This forces the LSB radix tree to split by trailing bits.
    for i in 256..456 {
        array.insert(i);
    }

    // Verify all hits
    for i in 256..456 {
        assert!(array.contains(i), "missing {}", i);
    }

    // Verify misses (values outside the range but same bit-width)
    for i in 456..556 {
        assert!(!array.contains(i), "false positive for {}", i);
    }

    // Verify misses in different bit-widths
    assert!(!array.contains(0));
    assert!(!array.contains(1));
    assert!(!array.contains(9999));
    assert!(!array.contains(u64::MAX));
}

#[test]
fn test_lsb_radix_extreme_same_trailing_bits() {
    // Worst case for LSB routing: many values with identical trailing bits.
    // e.g. all values end in 0b0000 (multiples of 16).
    // The radix tree must still correctly split and find them.

    let mut array = Bwspi::new();

    // Insert 200 multiples of 16 (all share trailing 4 bits = 0000)
    for i in 0..200u64 {
        array.insert((i + 16) * 16); // values 256, 272, 288, ...
    }

    // Verify all hits
    for i in 0..200u64 {
        let val = (i + 16) * 16;
        assert!(array.contains(val), "missing {}", val);
    }

    // Verify misses (odd numbers can't be multiples of 16)
    for i in 0..100u64 {
        let val = (i + 16) * 16 + 1;
        assert!(!array.contains(val), "false positive for {}", val);
    }
}
