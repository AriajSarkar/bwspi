# Architecture

> How Sarkar Bucket Array (SBA) / BWSPI is structured internally.

---

## The Problem SBA Solves

Traditional hash maps spend CPU cycles forcing artificial uniform distribution:

```
HashMap:  value → hash(value) → bucket     60+ CPU instructions
SBA:     value → bit_width(value) → bucket  1 CPU instruction
```

SBA skips the hash entirely. It uses a **physical property** of the data — its binary bit-width — as a free routing key.

---

## Two-Table Design

SBA uses two complementary structures:

```
Table 1: Data Store               Table 2: Bit-Width Index
┌─────────────────────┐           ┌──────────────────────────┐
│ idx 0 → 42          │◄──────────│ bw=2:  [3]               │
│ idx 1 → 7           │◄──────────│ bw=3:  [1]               │
│ idx 2 → 1000        │◄──────────│ bw=6:  [0]               │
│ idx 3 → 3           │◄──────────│ bw=10: [2]               │
│                     │           │                          │
│ append-only Vec     │           │ dynamic Vec<Vec<usize>>  │
│ never sorted        │           │ grows/shrinks on demand  │
│ never moved         │           │                          │
└─────────────────────┘           └──────────────────────────┘
```

### Table 1 — Data Store (`Vec<u64>`)

- **Append-only**: elements pushed to the end, never moved or sorted
- **Contiguous**: excellent CPU cache locality for sequential access
- **Stable indices**: once inserted at index N, it stays at index N forever

### Table 2 — Bucket Index (`Vec<Vec<usize>>`)

- **Sparse**: only allocates slots for bit-widths that exist in the data
- **Dynamic**: starts at **zero slots**, grows when wider values arrive
- **Shrinkable**: trims trailing empty buckets after deletion
- **Pointers, not copies**: each bucket stores data-store indices, not data copies

---

## Data Flow

### Insert

```
insert(42)
  │
  ├─ 1. Append to data store     → data = [..., 42]   index = 5
  │
  ├─ 2. Compute bit-width        → 42 = 0b101010 → bw = 6
  │     (1 LZCNT instruction)
  │
  ├─ 3. Grow bucket array if     → ensure buckets[0..6] exist
  │     bw >= current length
  │
  └─ 4. Push index to bucket     → buckets[6].push(5)
```

### Search

```
contains(42)
  │
  ├─ 1. Compute bit-width        → bw = 6
  │
  ├─ 2. Check bucket exists      → buckets.len() > 6? yes
  │     (if no → instant false)
  │
  └─ 3. Scan bucket[6] only      → for &idx in bucket[6]:
        (not the entire dataset)      data[idx] == 42? → found!
```

### Delete

```
remove(42)
  │
  ├─ 1. Compute bit-width        → bw = 6
  │
  ├─ 2. Find in bucket[6]        → position where data[idx] == 42
  │
  ├─ 3. swap_remove from bucket  → O(1) removal from bucket vec
  │     (data store untouched)
  │
  └─ 4. Shrink trailing empties  → if highest buckets now empty, pop them
```

---

## Dynamic Bucket Sizing

The bucket array is **not fixed at 65 slots**. It adapts to the data:

```
Data: [5]              → buckets has 4 slots   (bw 0..3)
Data: [5, 1000]        → buckets has 11 slots  (bw 0..10)
Data: [5, 1000, 2^63]  → buckets has 64 slots  (bw 0..63)

After removing 2^63:   → buckets shrinks back to 11 slots
```

### Growth: `ensure_bucket(bw)`

```rust
if bw >= self.buckets.len() {
    self.buckets.resize_with(bw + 1, Vec::new);
}
```

New slots are empty `Vec`s — zero heap allocation until something is inserted into them.

### Shrinkage: `shrink_trailing()`

```rust
while self.buckets.last().map_or(false, |b| b.is_empty()) {
    self.buckets.pop();
}
```

Only trims from the end. Middle gaps (empty buckets between occupied ones) stay as empty `Vec`s — this is by design, since the index must be directly addressable by bit-width.

### Real Slot Counts

| Data Pattern | Active Buckets | Total Slots | Wasted Slots |
|---|---|---|---|
| All 8-bit values | ~9 | 9 | 0 |
| All 32-bit values | 1 | 33 | 32 |
| Sequential 1..1K | 10 | 11 | 1 |
| High entropy (1M random) | 65 | 65 | 0 |
| 5 clustered ranges | 8 | 62 | 54 |

---

## Memory Layout

### What's on the heap

```
Bwspi {
    data: Vec<u64>               → [u64; capacity] on heap
    buckets: Vec<Vec<usize>>     → [Vec<usize>; capacity] on heap
                                    └→ each inner Vec: [usize; capacity] on heap
}
```

### Overhead formula

```
Total heap = (data.capacity × 8 bytes)
           + (buckets.capacity × 24 bytes)      ← outer Vec metadata
           + Σ(bucket[i].capacity × 8 bytes)     ← inner index storage
```

### Measured overhead

| Scale | SBA Heap | HashMap Heap | SBA vs raw data |
|---|---|---|---|
| 1K | 21.0 KB | 34.0 KB | 2.7x |
| 100K | 1.90 MB | 2.13 MB | 2.5x |
| 1M | 15.94 MB | 34.00 MB | 2.1x |

The ratio improves at scale because the bucket index (fixed ~65 entries) becomes a smaller fraction of total storage.

---

## Struct Definition

```rust
pub struct Bwspi {
    data: Vec<u64>,              // Table 1: append-only data store
    buckets: Vec<Vec<usize>>,    // Table 2: bit-width → data indices
}
```

That's it. Two fields. ~250 lines of implementation. Zero dependencies.
