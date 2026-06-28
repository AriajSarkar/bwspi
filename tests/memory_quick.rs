//! Quick targeted memory diagnostic for the missing high-entropy data.
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
    let slots = bwspi.bucket_slots();

    // Compute fast_table bytes from distribution (matches new code)
    let mut ft_bytes = 0usize;
    #[allow(unused_variables)]
    let mut ft_slots = 0usize;
    for w in 0..slots {
        let k = bwspi.bucket_size(w);
        if k > 64 { // Fix 1: only built for > LEAF_CAP
            // Fix 3: ~70% load factor
            let cap = (k * 10 / 7).next_power_of_two().max(8);
            ft_slots += cap;
            ft_bytes += cap * 16; // sizeof (u64, usize) = 16
        }
    }

    println!("N={:>9} | total={:>10} ({:>6.2} MB) | raw_data={:>10} ({:>6.2} MB) | fast_table={:>10} ({:>6.2} MB, {:>5.1}%) | overhead={:.1}x | {:.1} B/elem",
        n,
        total, total as f64 / 1_048_576.0,
        data_bytes, data_bytes as f64 / 1_048_576.0,
        ft_bytes, ft_bytes as f64 / 1_048_576.0, 100.0 * ft_bytes as f64 / total as f64,
        total as f64 / data_bytes as f64,
        total as f64 / n as f64);
}

#[test]
fn quick_memory_report() {
    println!("\n--- HIGH ENTROPY ---");
    for &n in &[1_000, 10_000, 50_000, 100_000, 150_000, 1_000_000] {
        report(n);
    }
    println!();
}
