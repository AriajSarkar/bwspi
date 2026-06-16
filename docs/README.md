# SBA / BWSPI Documentation

> Sarkar Bucket Array (SBA) / Bit-Width Sparse Pointer Index (BWSPI)

## Contents

| Document | What You'll Learn |
|----------|-------------------|
| [Architecture](architecture.md) | Two-table design, data flow, dynamic bucket sizing |
| [How It Works](how-it-works.md) | Bit-width routing, the math behind search reduction |
| [Benchmarks](benchmarks.md) | Performance data vs HashMap, SwissTable, FxHash, AHash |
| [SIMD Internals](simd.md) | AVX2 vectorized bucket scan, intrinsics breakdown |
| [Trade-offs](trade-offs.md) | When to use SBA, when not to, comparison matrix |

## Quick Start

```rust
use bwspi::Bwspi;

let mut index = Bwspi::new();
index.insert(42);
index.insert(1000);

assert!(index.contains(42));       // routes to bit-width 6 bucket
assert!(!index.contains(999));     // instant false if bucket empty
```

See the [README](../README.md) for full API examples.
