# Trade-offs

> When to use Sarkar Bucket Array (SBA) / BWSPI, when not to, and why.

---

## Decision Matrix

| Workload | SBA | HashMap | Winner |
|---|---|---|---|
| Write-heavy (lots of inserts) | ✅ 6 ns/insert | ❌ 31 ns/insert | **SBA** (5× faster) |
| Lookup-heavy (lots of reads, few writes) | ⚠️ O(k) bucket scan | ✅ O(1) amortized | **HashMap** |
| Miss-heavy (most searches fail) | ✅ 1-4 ns instant miss | ❌ 10-20 ns hash + probe | **SBA** |
| Latency-critical (no spikes allowed) | ✅ No rehash ever | ❌ O(n) rehash spikes | **SBA** |
| Memory-constrained | ✅ ~2x overhead | ❌ ~4x overhead | **SBA** |
| High-entropy data (varied bit-widths) | ✅ 98% search reduction | ✅ O(1) | Tie |
| Uniform-width data (same bit-width) | ❌ Degrades to linear | ✅ O(1) | **HashMap** |
| Duplicate values | ✅ Native support | ❌ Overwrites | **SBA** |
| Key-value pairs | ❌ Not a map | ✅ Built for this | **HashMap** |
| String / composite keys | ❌ u64 only | ✅ Any Hashable type | **HashMap** |
| DoS resistance needed | ⚠️ N/A (no hash) | ✅ SipHash | **HashMap** |

---

## ✅ Use SBA When

### 1. Append-Only Data Streams
Logs, telemetry, event sourcing — data that's written once and queried occasionally. SBA's append-only data store is a natural fit.

### 2. High-Entropy Integer Data
Random IDs, mixed-size measurements, sensor readings — data with naturally varied bit-widths gives 95-98% search reduction.

### 3. Latency-Sensitive Systems
Real-time processing, game engines, trading systems — anywhere a HashMap rehash spike (potentially milliseconds) would be catastrophic.

### 4. Memory-Tight Environments
Embedded systems, microservices with memory limits — SBA uses roughly half the heap of HashMap.

### 5. Duplicate-Heavy Datasets
Frequency counting, multi-value indices — SBA stores every occurrence with its own index. HashMap would overwrite.

### 6. Write-Heavy Workloads
High-throughput ingestion — SBA inserts at 6 ns/element (1M scale), HashMap at 31 ns.

### 7. No-std / Embedded
SBA has zero dependencies. The core is pure Rust with no allocator tricks.

---

## ❌ Don't Use SBA When

### 1. All Values Same Bit-Width
If every value is between 2^31 and 2^32, they all route to bucket 32. Search becomes linear scan. HashMap is strictly better here.

### 2. Lookup-Dominated Workloads
If you insert once and lookup millions of times, HashMap's O(1) amortized lookup beats SBA's O(k) bucket scan, especially at large N.

### 3. You Need Key-Value Storage
SBA is a set/index. It answers "is X present?" and "where is X?". It doesn't map keys to values. Use HashMap or BTreeMap for that.

### 4. Non-Integer Keys
SBA operates on `u64`. Strings, structs, or composite keys need hashing. Though you could hash them to u64 and use SBA as a secondary index.

### 5. Adversarial Input
If attackers can control the data and try to degrade performance (HashDoS), SBA's bit-width routing is predictable. They could force all values to the same bit-width. SipHash-backed HashMap is designed for this.

---

## Comparison: SBA vs Every Hash Map Variant

| Property | SBA | HashMap (std) | SwissTable | AHash | FxHash |
|---|---|---|---|---|---|
| **Routing** | 1 inst (LZCNT) | ~60 inst (SipHash) | ~15 inst (foldhash) | ~15 inst (AES-NI) | ~5 inst (mul+shift) |
| **Lookup (1K)** | 2.66 ns | 10.28 ns | 1.64 ns | 2.38 ns | 1.90 ns |
| **Insert (1M)** | 6.71 ms | 31.91 ms | — | — | — |
| **Heap (1M)** | 15.94 MB | 34.00 MB | ~34 MB | ~34 MB | ~34 MB |
| **Rehash spikes** | Never | Yes | Yes | Yes | Yes |
| **Duplicates** | Native | Overwrite | Overwrite | Overwrite | Overwrite |
| **DoS resistant** | No | Yes | No | Yes | No |
| **Worst case** | O(n) if same bw | O(n) hash collision | O(n) | O(n) | O(n) |

---

## The Honest Take

SBA is **not a HashMap replacement**. It's a different tool for a different job.

HashMap is a general-purpose key-value store with O(1) amortized everything. It works on any hashable type. It's battle-tested and DoS-resistant (with SipHash).

SBA is a **specialized integer index** that trades theoretical lookup guarantees for:
- Zero routing cost (1 instruction vs 60+)
- Zero rehash latency spikes
- Half the memory
- Native duplicate support
- 5× faster insertion

If your data is unsorted u64 values with varied bit-widths and you care about insertion speed, memory, and predictable latency — SBA is the better tool.

If you need key-value pairs, O(1) lookup at any scale, or string keys — use HashMap.
