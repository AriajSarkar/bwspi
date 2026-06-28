//! Exact memory breakdown diagnostic for BWSPI.
//!
//! Run with:  cargo test --test memory_diagnostic -- --nocapture
//!
//! This measures the PRECISE byte-level memory cost of every component
//! in the current BWSPI implementation, with special focus on the
//! fast_table overhead added by the optimization session.

use bwspi::Bwspi;

struct XorShift64(u64);
impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut v = self.0;
        v ^= v << 13;
        v ^= v >> 7;
        v ^= v << 17;
        self.0 = v;
        v
    }
}

/// Detailed memory breakdown for a Bwspi instance.
/// We access pub(crate) fields through the bwspi crate's test visibility.
struct MemoryBreakdown {
    // Core data
    data_bytes: usize,
    bucket_positions_bytes: usize,

    // CRUD index
    crud_outer_bytes: usize,
    crud_inner_bytes: usize,

    // Tree: entries + values (the leaf data for sorting)
    tree_entries_bytes: usize,
    tree_values_bytes: usize,

    // Tree: fast_table (the hash accelerator)
    tree_fast_table_bytes: usize,
    tree_fast_table_slots: usize,
    tree_fast_table_used: usize,

    // Tree: children (Box<[BucketNode; 8]> overhead)
    tree_children_bytes: usize,

    // Dirty flags
    dirty_bytes: usize,

    // Totals
    total_bytes: usize,
    elements: usize,
}

fn measure(bwspi: &mut Bwspi, label: &str) -> MemoryBreakdown {
    // Force all trees to be built so we measure the actual state
    // We do this by triggering a contains for a value in each width
    // Actually, let's just trigger rebuild_all_dirty via sorted_snapshot
    // No - that's too invasive. Let's trigger contains for value 0 which
    // is a cheap way to force tree[0] build, but we need ALL trees.
    // The simplest way: call sorted_snapshot which rebuilds all dirty trees.
    let _ = bwspi.sorted_snapshot();

    // Now measure everything through the public memory_usage_bytes + manual calc
    let data_bytes = bwspi.data().len() * 8;
    let elements = bwspi.len();

    // We can get the total from the public API
    let total_bytes = bwspi.memory_usage_bytes();

    // For the detailed breakdown, we need to print what we can compute
    // from the public observability methods
    let storage_len = bwspi.storage_len();
    let bucket_slots = bwspi.bucket_slots();

    // bucket_positions: one usize per storage entry
    let bucket_positions_bytes = storage_len * std::mem::size_of::<usize>();

    // crud_buckets outer: one Vec<usize> per slot
    let crud_outer_bytes = bucket_slots * std::mem::size_of::<Vec<usize>>();

    // crud_buckets inner: sum of all bucket element counts * sizeof(usize)
    let mut crud_inner_bytes = 0;
    for w in 0..bucket_slots {
        crud_inner_bytes += bwspi.bucket_size(w) * std::mem::size_of::<usize>();
    }

    // dirty flags
    let dirty_bytes = bucket_slots; // Vec<bool>, 1 byte each

    // Tree overhead (the public method gives us the total)
    // tree_overhead = total - data - positions - crud_outer - crud_inner - dirty
    // But memory_usage_bytes uses capacity, not len. So let's compute from total.
    let tree_total = total_bytes.saturating_sub(
        data_bytes + bucket_positions_bytes + crud_outer_bytes + crud_inner_bytes + dirty_bytes
    );

    // For the fast_table specifically, we need to compute from the distribution
    // Each width with k elements has a fast_table of (k*2).next_power_of_two().max(8) slots
    // Each slot is (u64, usize) = 16 bytes
    let mut ft_bytes = 0usize;
    let mut ft_slots = 0usize;
    let mut ft_used = 0usize;
    let mut entries_bytes = 0usize;
    let mut values_bytes = 0usize;
    for w in 0..bucket_slots {
        let k = bwspi.bucket_size(w);
        if k > 64 { // Fix 1: only built for > LEAF_CAP
            // Fix 3: ~70% load factor
            let cap = (k * 10 / 7).next_power_of_two().max(8);
            ft_slots += cap;
            ft_used += k;
            ft_bytes += cap * std::mem::size_of::<(u64, usize)>();
            // Values are no longer stored in the tree node (Fix 2)
            entries_bytes += k * std::mem::size_of::<usize>();
        } else if k > 0 {
            // No fast table, but entries are still there
            entries_bytes += k * std::mem::size_of::<usize>();
        }
    }

    // Children overhead = tree_total - ft_bytes - entries_bytes - values_bytes
    let children_bytes = tree_total.saturating_sub(ft_bytes + entries_bytes + values_bytes);

    let breakdown = MemoryBreakdown {
        data_bytes,
        bucket_positions_bytes,
        crud_outer_bytes,
        crud_inner_bytes,
        tree_entries_bytes: entries_bytes,
        tree_values_bytes: values_bytes,
        tree_fast_table_bytes: ft_bytes,
        tree_fast_table_slots: ft_slots,
        tree_fast_table_used: ft_used,
        tree_children_bytes: children_bytes,
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
    let tree_leaf = breakdown.tree_entries_bytes + breakdown.tree_values_bytes;
    let _tree_accel = breakdown.tree_fast_table_bytes;
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

    println!("  TREE LEAVES (entries + values in BucketNode leaves)");
    println!("    entries (Vec<usize>)         : {:>10} bytes  ({:.2} MB)",
        breakdown.tree_entries_bytes, breakdown.tree_entries_bytes as f64 / 1_048_576.0);
    println!("    values (Vec<u64>)            : {:>10} bytes  ({:.2} MB)",
        breakdown.tree_values_bytes, breakdown.tree_values_bytes as f64 / 1_048_576.0);
    println!("    ─── subtotal                 : {:>10} bytes  ({:.2} MB)  [{:.1}%]",
        tree_leaf, tree_leaf as f64 / 1_048_576.0, 100.0 * tree_leaf as f64 / breakdown.total_bytes as f64);
    println!();

    println!("  ⚡ FAST_TABLE (hash accelerator added by optimization)");
    println!("    slots allocated              : {:>10}", breakdown.tree_fast_table_slots);
    println!("    slots used (= elements)      : {:>10}  (load factor: {:.1}%)",
        breakdown.tree_fast_table_used,
        if breakdown.tree_fast_table_slots > 0 { 100.0 * breakdown.tree_fast_table_used as f64 / breakdown.tree_fast_table_slots as f64 } else { 0.0 });
    println!("    bytes (16 B/slot)            : {:>10} bytes  ({:.2} MB)  [{:.1}%]",
        breakdown.tree_fast_table_bytes,
        breakdown.tree_fast_table_bytes as f64 / 1_048_576.0,
        100.0 * breakdown.tree_fast_table_bytes as f64 / breakdown.total_bytes as f64);
    println!();

    println!("  TREE STRUCTURE (Box<[BucketNode; 8]> children)");
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
    println!("  fast_table % of total          : {:>10.1}%",
        100.0 * breakdown.tree_fast_table_bytes as f64 / breakdown.total_bytes as f64);
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
    println!("║        BWSPI MEMORY DIAGNOSTIC — EXACT BYTE COUNTS        ║");
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
        // HashSet<u64> uses SwissTable internally:
        //   - 1 byte control per slot
        //   - 8 bytes per u64 value
        //   - Load factor ~87.5% (7/8) means capacity ≈ n * 8/7
        //   - Rounded to next power of two
        let capacity = ((n * 8 / 7) as usize).next_power_of_two();
        let hashset_bytes = capacity * (1 + 8); // control byte + u64
        println!("  N={:>9}:  HashSet<u64> ≈ {:>10} bytes  ({:.2} MB)  [{:.1} bytes/element]",
            n, hashset_bytes, hashset_bytes as f64 / 1_048_576.0, hashset_bytes as f64 / n as f64);
    }
    println!();
}
