//! Quick targeted memory diagnostic for BWSPI (LSB radix tree).
//! Run with:  cargo test --release --test memory_quick -- --nocapture

use bwspi::Bwspi;

struct Xor(u64);
impl Xor {
    fn next(&mut self) -> u64 { let mut v=self.0; v^=v<<13; v^=v>>7; v^=v<<17; self.0=v; v }
}

fn gen(n: usize) -> Vec<u64> {
    let mut r = Xor(0xdead_beef_cafe_babe);
    (0..n).map(|_| {
        let b = (r.next() % 65) as u32;
        if b==0{0} else if b==64{r.next()} else { let mn=1u64<<(b-1); let rng=1u64<<b; mn+r.next()%(rng-mn) }
    }).collect()
}

fn report(n: usize) {
    let data = gen(n);
    let mut bwspi = Bwspi::with_capacity(n);
    bwspi.insert_bulk(&data);
    let _ = bwspi.sorted_snapshot(); // force tree build

    let total = bwspi.memory_usage_bytes();
    let data_bytes = n * 8;

    // No more fast_table — just measure total tree overhead
    let tree_overhead = total.saturating_sub(data_bytes + n * 8 + n * 8); // data + positions + crud_inner approx

    println!("N={:>9} | total={:>10} ({:>6.2} MB) | raw_data={:>10} ({:>6.2} MB) | tree_overhead={:>10} ({:>6.2} MB, {:>5.1}%) | overhead={:.1}x | {:.1} B/elem",
        n,
        total, total as f64 / 1_048_576.0,
        data_bytes, data_bytes as f64 / 1_048_576.0,
        tree_overhead, tree_overhead as f64 / 1_048_576.0, 100.0 * tree_overhead as f64 / total as f64,
        total as f64 / data_bytes as f64,
        total as f64 / n as f64);
}

#[test]
fn quick_memory_report() {
    println!("\n--- HIGH ENTROPY (LSB Radix Tree) ---");
    for &n in &[1_000, 10_000, 50_000, 100_000, 150_000, 1_000_000] {
        report(n);
    }
    println!();
}
