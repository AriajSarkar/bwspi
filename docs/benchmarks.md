# Benchmarks

> Performance data for Sarkar Bucket Array (SBA) / BWSPI measured against real competitors.

All benchmarks run with `RUSTFLAGS="-C target-cpu=native"`.

---

## Competitors

| Name | Hash Function | Implementation | Notes |
|---|---|---|---|
| **SBA / BWSPI** | None (LZCNT) | This crate | 1-instruction routing |
| **SwissTable** | foldhash | `hashbrown` v0.17.1 | Google SwissTable (backs Rust's std) |
| **HashMap (std)** | SipHash-1-3 | `std::collections` | DoS-resistant, slower hash |
| **AHashMap** | AES-NI | `ahash` v0.8 | Hardware-accelerated hashing |
| **FxHashMap** | Multiply+shift | `rustc-hash` v2 | Fastest integer hash, no DoS protection |
| **Linear scan** | None | Raw `Vec<u64>` | Full scan baseline |

---

## Lookup HIT — Criterion (100 samples, 5s each)

### N = 1,000 — High Entropy

| Structure | Time (ns) | vs SBA |
|---|---|---|
| SwissTable | 1.64 | 0.6x (faster) |
| FxHash | 1.90 | 0.7x (faster) |
| AHash | 2.38 | 0.9x (faster) |
| **SBA scalar** | **2.66** | **1.0x** |
| **SBA + SIMD** | **2.76** | **1.0x** |
| HashMap (std) | 10.28 | 3.9x slower |
| Linear scan | 137.68 | 51.8x slower |

> SBA is within 0.3 ns of AHash and **3.9× faster than std HashMap**.

---

## Insertion — Production Suite (Counting Allocator)

### N = 1,000,000 — High Entropy

| Structure | Total | Per Element | Heap Used | Allocations |
|---|---|---|---|---|
| **SBA / BWSPI** | **6.71 ms** | **6 ns** | **15.94 MB** | 849 |
| HashMap (std) | 31.91 ms | 31 ns | 34.00 MB | 1 |
| Vec (append) | 1.90 ms | 1 ns | 7.63 MB | 1 |

> **4.8× faster insertion** than HashMap. **47% of HashMap's heap.**

---

## Search — Cold vs Hot Cache

### N = 1,000 — High Entropy (Production Suite)

| Structure | Cold (ns) | Hot (ns) | Cold/Hot Ratio |
|---|---|---|---|
| **SBA** | 36 | 38 | ~1.0x |
| HashMap | 22 | 35 | ~0.6x |
| Linear | 363 | 414 | ~0.9x |

| Structure | Miss Cold (ns) | Miss Hot (ns) |
|---|---|---|
| **SBA** | **76** | **80** |
| HashMap | 199 | 205 |
| Linear | 8,100 | 5,000 |

> SBA miss detection is **2.5× faster** than HashMap (80 ns vs 205 ns).

---

## Heap Memory (Counting Allocator)

| N | SBA | HashMap | Vec (raw) | SBA Overhead |
|---|---|---|---|---|
| 1K | 21.0 KB | 34.0 KB | 7.8 KB | 2.7x |
| 10K | 201.9 KB | 340.0 KB | 78.1 KB | 2.6x |
| 100K | 1.90 MB | 2.13 MB | 781.2 KB | 2.5x |
| 1M | 15.94 MB | 34.00 MB | 7.63 MB | 2.1x |

---

## Search Reduction by Distribution

| Distribution | N | Worst Bucket | Reduction |
|---|---|---|---|
| High entropy | 1,000,000 | 19,251 | **98.1%** |
| Clustered (5 ranges) | 100,000 | 20,230 | **79.8%** |
| Sequential IDs | 100,000 | 34,465 | **65.5%** |
| Uniform 32-bit | 1,000 | 1,000 | 0% |

---

## CPU Routing Cost

```
SipHash (std):    ████████████████████████████████████████  ~60 instructions
AES-NI (aHash):   ████████████                              ~15 instructions
FxHash:           ████                                       ~5 instructions
SBA (LZCNT):      █                                          1 instruction
```

---

## Running Benchmarks Yourself

```bash
# Criterion — statistically rigorous (SIMD + SwissTable focused)
RUSTFLAGS="-C target-cpu=native" cargo bench --bench simd_bench

# Criterion — full comparison suite
RUSTFLAGS="-C target-cpu=native" cargo bench --bench bwspi_benchmarks

# Production suite — heap measurement, cold/hot cache, all distributions
RUSTFLAGS="-C target-cpu=native" cargo run --release
```

On Windows PowerShell:
```powershell
$env:RUSTFLAGS="-C target-cpu=native"; cargo bench --bench simd_bench
```
