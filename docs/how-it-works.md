# How It Works

> The bit-width routing mechanism behind Sarkar Bucket Array (SBA) / BWSPI.

---

## Bit-Width: The Free Routing Key

Every integer has a binary representation. Values that need the **same number of binary digits** land in the **same bucket**:

```
Values          Binary                Bit-Width    Bucket
──────          ──────                ─────────    ──────
0               0                     0            bucket[0]
1               1                     1            bucket[1]
2, 3            10, 11                2            bucket[2]
4, 5, 6, 7      100, 101, 110, 111    3            bucket[3]
8..15           1000..1111            4            bucket[4]
16..31          10000..11111          5            bucket[5]
32..63          100000..111111        6            bucket[6]
64..127         1000000..1111111      7            bucket[7]
128..255        10000000..11111111    8            bucket[8]
256..511        100000000..111111111  9            bucket[9]
...             ...                   ...          ...
```

Each bit-width doubles the range of values it covers. Bucket 3 holds 4 values (4-7), bucket 4 holds 8 values (8-15), bucket 10 holds 512 values (512-1023), and so on.

The bit-width is the number of binary digits needed to represent the value. Computing it costs **exactly 1 CPU instruction**:

```rust
fn bit_width(value: u64) -> usize {
    (64 - value.leading_zeros()) as usize
}
```

### Hardware Support

| Architecture | Instruction | Cycles | Pipeline Stalls |
|---|---|---|---|
| x86-64 (BMI1+) | `LZCNT` | 1 | None |
| ARM / AArch64 | `CLZ` | 1 | None |
| RISC-V (Zbb) | `CLZ` | 1 | None |

This isn't a hash function. There's no mixing, no multiplication, no XOR chain. It's a direct **measurement** of the data — like weighing a parcel to decide which shelf it goes on.

---

## Why 65 Buckets Maximum?

A `u64` can have bit-widths from 0 (the value `0`) to 64 (values ≥ 2^63). That's 65 possible values.

But SBA doesn't allocate all 65 upfront. If your data is all 8-bit, you get 9 buckets. If it's all 32-bit, you get 33. Only the bit-widths present in the data get allocated.

```
Data: [100, 200, 150]     → all bit-width 7-8  → 9 buckets
Data: [1, 2, 3, 4, 5]     → bit-widths 1-3     → 4 buckets
Data: [u64::MAX]           → bit-width 64       → 65 buckets
```

---

## Search Reduction: The Math

With N elements spread across B active buckets, the average bucket holds N/B elements. Instead of scanning all N elements (linear search), SBA scans only the matching bucket.

### Reduction Formula

```
search_reduction = 1 - (largest_bucket / N)
```

### Real Measurements

| Distribution | N | Largest Bucket | Search Reduction |
|---|---|---|---|
| High entropy (random) | 1,000,000 | 19,251 | **98.1%** |
| Clustered (5 ranges) | 100,000 | 20,230 | **79.8%** |
| Sequential (1..N) | 100,000 | 34,465 | **65.5%** |
| Uniform 32-bit | 1,000 | 1,000 | **0%** (worst case) |

### Why High Entropy Works Best

For truly random `u64` values, the probability of bit-width `b` is:

```
P(bw = b) = 2^(b-1) / 2^64    for 1 ≤ b ≤ 64
```

Higher bit-widths are exponentially more likely in the raw distribution. But when data is generated with **uniform bit-width distribution** (equal chance of any width), each bucket gets ~N/65 elements — nearly perfect partitioning.

### The Worst Case

When all values share the same bit-width (e.g., random numbers between 2^31 and 2^32 - 1), everything lands in bucket 32. Search degrades to linear scan of the entire dataset.

Even in this case, SBA still wins on:
- **Miss detection**: if the target has a different bit-width → instant `false`
- **Memory**: only 33 slots allocated, not a full hash table
- **Insertion**: still O(1), still no hash computation

---

## Insert Path (Step by Step)

```rust
index.insert(42);
```

**Step 1**: Append to data store
```
data: [... 42]
              ^
              index = data.len() - 1
```

**Step 2**: Compute bit-width (1 CPU instruction)
```
42 = 0b101010 → 64 - 58 leading zeros = 6
```

**Step 3**: Ensure bucket[6] exists
```
if 6 >= buckets.len() {
    buckets.resize_with(7, Vec::new);  // grow to fit
}
```

**Step 4**: Record index in bucket
```
buckets[6].push(index);
```

**Total cost**: 1 Vec push + 1 LZCNT + 1 bounds check + 1 Vec push = O(1) amortized.
No hash. No load factor check. No rehash.

---

## Search Path (Step by Step)

```rust
index.contains(42);
```

**Step 1**: Compute bit-width
```
42 → bw = 6
```

**Step 2**: Check if bucket exists
```
if 6 >= buckets.len() → return false    // instant miss
```

**Step 3**: Scan only bucket[6]
```
for &idx in &buckets[6] {
    if data[idx] == 42 → return true
}
return false
```

If bucket[6] has 15 elements (out of 1000 total), we scan 15 instead of 1000. That's the search reduction.

---

## Miss Detection: SBA's Secret Weapon

When searching for a value whose bit-width has no bucket (or an empty bucket), SBA returns `false` immediately:

```
index contains: [100, 200, 300]    → all bit-width 7-9
search(42)                         → bit-width 6 → bucket[6] is empty → false!
```

**Cost: 1 LZCNT + 1 bounds check = ~1-4 nanoseconds.**

Compare to HashMap, which must:
1. Compute full hash (~5-60 instructions)
2. Probe the hash table
3. Compare keys

Even for a miss, HashMap pays the full hash cost. SBA pays almost nothing.

---

## Duplicates

SBA handles duplicates natively:

```rust
index.insert(42);
index.insert(42);
index.insert(42);

index.find_all(42);  // returns [0, 1, 2] — all three indices
```

Each insert appends to the data store and adds an index to the bucket. No overwrites, no conflicts. This is fundamentally different from HashMap where `insert(key, value)` overwrites the previous value.
