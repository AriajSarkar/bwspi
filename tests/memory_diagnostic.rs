//! Exact memory breakdown diagnostic for BWSPI.
//!
//! Run with:  cargo test --test memory_diagnostic -- --nocapture
//!
//! This measures the PRECISE byte-level memory cost of every component
//! in the current BWSPI implementation using the LSB radix tree architecture.

use bwspi::Bwspi;

struct XorShift64(u64);
impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut v = self.0;
        v ^= v << 13;
        v >>= 7;
        v ^= v << 17;
        self.0 = v;
        v
    }
}

/// Detailed memory breakdown for a Bwspi instance.
struct MemoryBreakdown {
    // Core data
    data_bytes: usize,
    bucket_positions_bytes: usize,

    // CRUD index
    crud_outer_bytes: usize,
    crud_inner_bytes: usize,

    // Tree: entries (leaf storage IDs)
    tree_entries_bytes: usize,

    // Tree: children pointers (LSB radix routing)
    tree_children_bytes: usize,

    // Dirty flags
    dirty_bytes: usize,

    // Totals
    total_bytes: usize,
    elements: usize,
}

fn measure(bwspi: &mut Bwspi, label: &str) -> MemoryBreakdown {
    // Force all trees to be built
    let _ = bwspi.sorted_snapshot();

    let data_bytes = bwspi.data().len() * 8;
    let elements = bwspi.len();
    let total_bytes = bwspi.memory_usage_bytes();
    let storage_len = bwspi.storage_len();
    let bucket_slots = bwspi.bucket_slots();

    let bucket_positions_bytes = storage_len * std::mem::size_of::<usize>();
    let crud_outer_bytes = bucket_slots * std::mem::size_of::<Vec<usize>>();
    let mut crud_inner_bytes = 0;
    for w in 0..bucket_slots {
        crud_inner_bytes += bwspi.bucket_size(w) * std::mem::size_of::<usize>();
    }
    let dirty_bytes = bucket_slots;

    // Tree overhead = total - everything else
    let tree_total = total_bytes.saturating_sub(
        data_bytes + bucket_positions_bytes + crud_outer_bytes + crud_inner_bytes + dirty_bytes
    );

    // Entries bytes: each element appears once in tree leaves
    let mut tree_entries_bytes = 0;
    for w in 0..bucket_slots {
        let k = bwspi.bucket_size(w);
        if k > 0 {
            tree_entries_bytes += k * std::mem::size_of::<usize>();
        }
    }

    // Children bytes: the rest of tree overhead is routing pointers
    let tree_children_bytes = tree_total.saturating_sub(tree_entries_bytes);

    let breakdown = MemoryBreakdown {
        data_bytes,
        bucket_positions_bytes,
        crud_outer_bytes,
        crud_inner_bytes,
        tree_entries_bytes,
        tree_children_bytes,
        dirty_bytes,
        total_bytes,
        elements,
    };

    println!("\n{}", "=".repeat(60));
    println!("MEMORY BREAKDOWN: {} ({} elements)", label, breakdown.elements);
    println!("{}", "=".repeat(60));
    println!();

    let core = breakdown.data_bytes + breakdown.bucket_positions_bytes;
    let crud = breakdown.crud_outer_bytes + breakdown.crud_inner_bytes;
    let tree_leaf = breakdown.tree_entries_bytes;
    let tree_struct = breakdown.tree_children_bytes;
    let dirty = breakdown.dirty_bytes;

    println!("  CORE DATA (your original data store)");
    println!("    data (Vec<u64>)              : {:>10} bytes  ({:.2} MB)",
        breakdown.data_bytes, breakdown.data_bytes as f64 / 1_048_576.0);
    println!("    bucket_positions (Vec<usize>) : {:>10} bytes  ({:.2} MB)",
        breakdown.bucket_positions_bytes, breakdown.bucket_positions_bytes as f64 / 1_048_576.0);
    println!("    ─── subtotal                 : {:>10} bytes  ({:.2} MB)  [{:.1}%]",
        core, core as f64 / 1_048_576.0, 100.0 * core as f64 / breakdown.total_bytes as f64);
    println!();

    println!("  CRUD INDEX (your original flat buckets)");
    println!("    outer Vec<Vec<usize>>        : {:>10} bytes", breakdown.crud_outer_bytes);
    println!("    inner bucket contents        : {:>10} bytes  ({:.2} MB)",
        breakdown.crud_inner_bytes, breakdown.crud_inner_bytes as f64 / 1_048_576.0);
    println!("    ─── subtotal                 : {:>10} bytes  ({:.2} MB)  [{:.1}%]",
        crud, crud as f64 / 1_048_576.0, 100.0 * crud as f64 / breakdown.total_bytes as f64);
    println!();

    println!("  LSB RADIX TREE (leaf entries — storage IDs only, NO value copies)");
    println!("    entries (Vec<usize>)         : {:>10} bytes  ({:.2} MB)  [{:.1}%]",
        tree_leaf, tree_leaf as f64 / 1_048_576.0, 100.0 * tree_leaf as f64 / breakdown.total_bytes as f64);
    println!();

    println!("  TREE ROUTING (children pointers — LSB radix routing)");
    println!("    ─── subtotal                 : {:>10} bytes  ({:.2} MB)  [{:.1}%]",
        tree_struct, tree_struct as f64 / 1_048_576.0, 100.0 * tree_struct as f64 / breakdown.total_bytes as f64);
    println!();

    println!("  DIRTY FLAGS                    : {:>10} bytes", dirty);
    println!();

    println!("  ═══════════════════════════════════════════════");
    println!("  TOTAL                          : {:>10} bytes  ({:.2} MB)",
        breakdown.total_bytes, breakdown.total_bytes as f64 / 1_048_576.0);
    println!("  Per element                    : {:>10.1} bytes",
        breakdown.total_bytes as f64 / breakdown.elements.max(1) as f64);
    println!("  Raw data alone                 : {:>10.1} bytes/element",
        breakdown.data_bytes as f64 / breakdown.elements.max(1) as f64);
    println!("  Overhead ratio (total/raw)     : {:>10.2}x",
        breakdown.total_bytes as f64 / breakdown.data_bytes.max(1) as f64);
    println!();

    breakdown
}

fn gen_high_entropy(n: usize) -> Vec<u64> {
    let mut rng = XorShift64(0xdead_beef_cafe_babe);
    (0..n).map(|_| {
        let bits = (rng.next() % 65) as u32;
        if bits == 0 { 0 }
        else if bits == 64 { rng.next() }
        else {
            let min = 1u64 << (bits - 1);
            let range = 1u64 << bits;
            min + rng.next() % (range - min)
        }
    }).collect()
}

fn gen_uniform_32bit(n: usize) -> Vec<u64> {
    let mut rng = XorShift64(0x1234_5678_9abc_def0);
    let lo = 1u64 << 31;
    let hi = (1u64 << 32) - 1;
    (0..n).map(|_| lo + rng.next() % (hi - lo + 1)).collect()
}

#[test]
fn memory_diagnostic_all_scales() {
    println!("\n\n╔══════════════════════════════════════════════════════════════╗");
    println!("║   BWSPI MEMORY DIAGNOSTIC — LSB RADIX TREE ARCHITECTURE   ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    for &n in &[1_000, 10_000, 50_000, 100_000, 150_000, 1_000_000] {
        let data = gen_high_entropy(n);
        let mut bwspi = Bwspi::with_capacity(n);
        bwspi.insert_bulk(&data);
        measure(&mut bwspi, &format!("HIGH ENTROPY, N={}", n));
    }

    println!("\n\n--- WORST CASE: ALL SAME BIT-WIDTH (uniform 32-bit) ---\n");

    for &n in &[1_000, 10_000, 100_000, 150_000] {
        let data = gen_uniform_32bit(n);
        let mut bwspi = Bwspi::with_capacity(n);
        bwspi.insert_bulk(&data);
        measure(&mut bwspi, &format!("UNIFORM 32-BIT, N={}", n));
    }

    // Compare with what a HashSet would use
    println!("\n\n--- REFERENCE: std HashSet<u64> memory estimate ---\n");
    for &n in &[1_000, 10_000, 100_000, 150_000, 1_000_000] {
        let capacity = ((n * 8 / 7) as usize).next_power_of_two();
        let hashset_bytes = capacity * (1 + 8);
        println!("  N={:>9}:  HashSet<u64> ≈ {:>10} bytes  ({:.2} MB)  [{:.1} bytes/element]",
            n, hashset_bytes, hashset_bytes as f64 / 1_048_576.0, hashset_bytes as f64 / n as f64);
    }
    println!();
}
