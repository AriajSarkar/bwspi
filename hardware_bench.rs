//! Root-level Windows hardware benchmark for BWSPI.
//!
//! Run with:
//!   cargo run --release --bin hardware_bench
//!
//! It writes `bench.md` in the repository root. It deliberately uses only the
//! Windows SDK APIs exposed by the OS and the Rust standard library; no driver,
//! kernel extension, or unapproved telemetry package is installed.

use std::collections::{BTreeSet, HashSet};
use std::hint::black_box;
use std::process::Command;
use std::time::{Duration, Instant};

use bwspi::Bwspi;

const DEFAULT_ELEMENTS: usize = 150_000;
const RUNS: usize = 7;
const LOOKUPS: usize = 20_000;

#[derive(Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}

#[derive(Clone)]
struct ResultRow {
    workload: &'static str,
    algorithm: &'static str,
    median: Duration,
    min: Duration,
    max: Duration,
    cpu_percent: Option<f64>,
    working_set_delta: isize,
    traffic_lower_bound: usize,
}

#[derive(Clone, Copy)]
struct Sample {
    elapsed: Duration,
    cpu_percent: f64,
    working_set_delta: isize,
}

fn median<T: Ord + Copy>(values: &mut [T]) -> T {
    values.sort_unstable();
    values[values.len() / 2]
}

fn median_f64(values: &mut [f64]) -> f64 {
    values.sort_by(|left, right| left.total_cmp(right));
    values[values.len() / 2]
}

fn benchmark<F>(
    workload: &'static str,
    algorithm: &'static str,
    traffic_lower_bound: usize,
    mut operation: F,
) -> ResultRow
where
    F: FnMut(),
{
    operation(); // warm allocator/code paths, excluded from results
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let before = telemetry::process_snapshot();
        let started = Instant::now();
        operation();
        let elapsed = started.elapsed();
        let after = telemetry::process_snapshot();
        samples.push(Sample {
            elapsed,
            cpu_percent: after.cpu_percent_since(before, elapsed),
            working_set_delta: after.working_set_delta(before),
        });
    }

    let mut elapsed: Vec<_> = samples.iter().map(|sample| sample.elapsed).collect();
    let mut cpu: Vec<_> = samples.iter().map(|sample| sample.cpu_percent).collect();
    let mut working_set: Vec<_> = samples
        .iter()
        .map(|sample| sample.working_set_delta)
        .collect();
    let median_elapsed = median(&mut elapsed);
    let min = *elapsed.first().expect("benchmark has samples");
    let max = *elapsed.last().expect("benchmark has samples");

    ResultRow {
        workload,
        algorithm,
        median: median_elapsed,
        min,
        max,
        cpu_percent: {
            let value = median_f64(&mut cpu);
            // GetProcessTimes is quantized by the scheduler on this Windows
            // build. For short CPU-bound samples a zero result means below its
            // observable resolution, not zero CPU use.
            (value > 0.0).then_some(value)
        },
        working_set_delta: median(&mut working_set),
        traffic_lower_bound,
    }
}

fn high_entropy(count: usize) -> Vec<u64> {
    let mut rng = XorShift64(0x9e37_79b9_7f4a_7c15);
    (0..count)
        .map(|_| match rng.next() % 5 {
            0 => rng.next() & 0xff,
            1 => (1_u64 << 20) | (rng.next() & 0x000f_ffff),
            2 => (1_u64 << 40) | (rng.next() & 0x0000_ffff_ffff),
            3 => (1_u64 << 63) | (rng.next() & 0x0000_ffff_ffff),
            _ => rng.next(),
        })
        .collect()
}

fn uniform_width(count: usize) -> Vec<u64> {
    let mut rng = XorShift64(0xd1b5_4a32_d192_ed03);
    (0..count)
        .map(|_| (1_u64 << 31) | (rng.next() & 0x7fff_ffff))
        .collect()
}

fn sample_targets(data: &[u64]) -> Vec<u64> {
    data.iter()
        .step_by((data.len() / LOOKUPS).max(1))
        .copied()
        .take(LOOKUPS)
        .collect()
}

fn run_stream_workloads(rows: &mut Vec<ResultRow>, label: &'static str, data: &[u64]) {
    let bytes = std::mem::size_of_val(data);
    let targets = sample_targets(data);

    rows.push(benchmark(label, "BWSPI insert", bytes * 2, || {
        let mut index = Bwspi::with_capacity(data.len());
        index.insert_bulk(black_box(data));
        black_box(index.len());
    }));
    rows.push(benchmark(label, "Vec push", bytes, || {
        let mut values = Vec::with_capacity(data.len());
        values.extend_from_slice(black_box(data));
        black_box(values.len());
    }));
    rows.push(benchmark(label, "HashSet insert", bytes * 2, || {
        let mut set = HashSet::with_capacity(data.len());
        set.extend(black_box(data).iter().copied());
        black_box(set.len());
    }));
    rows.push(benchmark(label, "BTreeSet insert", bytes * 2, || {
        let mut set = BTreeSet::new();
        set.extend(black_box(data).iter().copied());
        black_box(set.len());
    }));

    let mut index = {
        let mut value = Bwspi::with_capacity(data.len());
        value.insert_bulk(data);
        value
    };
    let vector = data.to_vec();
    let mut sorted = data.to_vec();
    sorted.sort_unstable();
    let hash: HashSet<u64> = data.iter().copied().collect();
    let tree: BTreeSet<u64> = data.iter().copied().collect();
    let lookup_bytes = targets.len() * std::mem::size_of::<u64>();

    rows.push(benchmark(label, "BWSPI contains", lookup_bytes, || {
        for &target in &targets {
            black_box(index.contains(black_box(target)));
        }
    }));
    rows.push(benchmark(
        label,
        "Vec linear contains",
        lookup_bytes,
        || {
            for &target in &targets {
                black_box(vector.contains(black_box(&target)));
            }
        },
    ));
    rows.push(benchmark(label, "Vec binary_search", lookup_bytes, || {
        for &target in &targets {
            let _ = black_box(sorted.binary_search(black_box(&target)));
        }
    }));
    rows.push(benchmark(label, "HashSet contains", lookup_bytes, || {
        for &target in &targets {
            black_box(hash.contains(black_box(&target)));
        }
    }));
    rows.push(benchmark(label, "BTreeSet contains", lookup_bytes, || {
        for &target in &targets {
            black_box(tree.contains(black_box(&target)));
        }
    }));
}

fn run_sort_workloads(rows: &mut Vec<ResultRow>, label: &'static str, data: &[u64]) {
    let bytes = std::mem::size_of_val(data);
    let mut snapshot_index = {
        let mut index = Bwspi::with_capacity(data.len());
        index.insert_bulk(data);
        index
    };
    rows.push(benchmark(
        label,
        "BWSPI sorted_snapshot (prebuilt index)",
        bytes * 2,
        || {
            black_box(snapshot_index.sorted_snapshot());
        },
    ));
    rows.push(benchmark(
        label,
        "BWSPI build + snapshot",
        bytes * 3,
        || {
            let mut index = Bwspi::with_capacity(data.len());
            index.insert_bulk(data);
            black_box(index.sorted_snapshot());
        },
    ));
    rows.push(benchmark(
        label,
        "Sarkar Sort (build + sort)",
        bytes * 3,
        || {
            let mut index = Bwspi::with_capacity(data.len());
            index.insert_bulk(data);
            black_box(index.sarkar_sort());
            assert!(index.data()[..index.len()].is_sorted());
        },
    ));
    rows.push(benchmark(label, "Vec sort_unstable", bytes * 2, || {
        let mut values = data.to_vec();
        values.sort_unstable();
        black_box(values);
    }));
    rows.push(benchmark(label, "Vec stable sort", bytes * 2, || {
        let mut values = data.to_vec();
        values.sort();
        black_box(values);
    }));
}

fn run_update_delete_workloads(rows: &mut Vec<ResultRow>, data: &[u64]) {
    let updates = (0..data.len()).step_by(97).collect::<Vec<_>>();
    let bytes = updates.len() * std::mem::size_of::<u64>();
    rows.push(benchmark(
        "CRUD, high entropy",
        "BWSPI update_at",
        bytes * 2,
        || {
            let mut index = Bwspi::with_capacity(data.len());
            index.insert_bulk(data);
            for &id in &updates {
                black_box(index.update_at(id, data[id] ^ (1_u64 << 63)));
            }
            black_box(index.len());
        },
    ));
    rows.push(benchmark(
        "CRUD, high entropy",
        "Vec indexed update",
        bytes,
        || {
            let mut values = data.to_vec();
            for &id in &updates {
                values[id] ^= 1_u64 << 63;
            }
            black_box(values);
        },
    ));
    rows.push(benchmark(
        "CRUD, high entropy",
        "BWSPI remove_at",
        bytes,
        || {
            let mut index = Bwspi::with_capacity(data.len());
            index.insert_bulk(data);
            for &id in &updates {
                black_box(index.remove_at(id));
            }
            black_box(index.len());
        },
    ));
    rows.push(benchmark(
        "CRUD, high entropy",
        "Vec tombstone write",
        bytes,
        || {
            let mut values = data.to_vec();
            for &id in &updates {
                values[id] = 0;
            }
            black_box(values);
        },
    ));
}

fn fmt_duration(duration: Duration) -> String {
    if duration.as_nanos() < 1_000 {
        format!("{} ns", duration.as_nanos())
    } else if duration.as_micros() < 1_000 {
        format!("{:.2} us", duration.as_nanos() as f64 / 1_000.0)
    } else if duration.as_millis() < 1_000 {
        format!("{:.2} ms", duration.as_secs_f64() * 1_000.0)
    } else {
        format!("{:.3} s", duration.as_secs_f64())
    }
}

fn fmt_bytes(value: usize) -> String {
    if value < 1024 {
        format!("{value} B")
    } else if value < 1024 * 1024 {
        format!("{:.2} KiB", value as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", value as f64 / (1024.0 * 1024.0))
    }
}

fn shell(command: &str) -> String {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .trim()
            .replace('|', "/"),
        Ok(output) => format!("unavailable (PowerShell exit {})", output.status),
        Err(error) => format!("unavailable ({error})"),
    }
}

fn system_inventory() -> String {
    shell(
        "$os=Get-CimInstance Win32_OperatingSystem; $cs=Get-CimInstance Win32_ComputerSystem; $cpu=Get-CimInstance Win32_Processor | Select-Object -First 1; $gpu=Get-CimInstance Win32_VideoController | Select-Object -First 1; \"OS=$($os.Caption) $($os.Version); system=$($cs.Manufacturer) $($cs.Model); CPU=$($cpu.Name), logical=$($cpu.NumberOfLogicalProcessors), maxMHz=$($cpu.MaxClockSpeed); GPU=$($gpu.Name); BIOS=$((Get-CimInstance Win32_BIOS).SMBIOSBIOSVersion)\"",
    )
}

fn acpi_temperature_celsius() -> String {
    let output = shell(
        "$t=Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty CurrentTemperature; if($null -eq $t){'unavailable'}else{[math]::Round(($t / 10.0) - 273.15,1)}",
    );
    if output == "unavailable" {
        "unavailable: no ACPI thermal zone exposed by firmware".to_owned()
    } else {
        format!(
            "{output} C (ACPI zone; Windows does not guarantee this is CPU package temperature)"
        )
    }
}

fn report(
    rows: &[ResultRow],
    element_count: usize,
    suite_started_temp: &str,
    suite_ended_temp: &str,
) -> String {
    let mut output = String::new();
    output.push_str("# Native BWSPI Benchmark\n\n");
    output.push_str("Generated by `cargo run --release --bin hardware_bench`. Each timing is the median of seven warm runs; it is a local measurement, not a cross-machine claim.\n\n");
    output.push_str("## Environment\n\n");
    output.push_str(&format!("- {}\n", system_inventory()));
    output.push_str(&format!("- Rust: {}\n", shell("rustc --version")));
    output.push_str(&format!("- Cargo: {}\n", shell("cargo --version")));
    output.push_str(&format!("- Crate: bwspi {}\n", env!("CARGO_PKG_VERSION")));
    output.push_str(&format!("- Elements per distribution: {element_count}; lookup targets: {LOOKUPS}; measured runs: {RUNS}.\n"));
    output.push_str(&format!(
        "- CPU features: lzcnt={}, avx2={}\n",
        cfg!(target_arch = "x86_64") && std::is_x86_feature_detected!("lzcnt"),
        cfg!(target_arch = "x86_64") && std::is_x86_feature_detected!("avx2")
    ));

    output.push_str("\n## Windows-native telemetry\n\n");
    output.push_str(&telemetry::platform_report());
    output.push_str(&format!(
        "- ACPI thermal reading before suite: {suite_started_temp}\n"
    ));
    output.push_str(&format!(
        "- ACPI thermal reading after suite: {suite_ended_temp}\n"
    ));
    output.push_str("- CPU package temperature is **not** available through a universal Windows API. Dell Command Monitor, LibreHardwareMonitor, or vendor drivers can expose it when already installed; this bench intentionally does not install drivers or infer a package temperature from ACPI.\n");
    output.push_str("- Per-core L1/L2 occupancy and hardware register pressure are **not** exposed as reliable Windows user-mode counters on this CPU. Cache sizes below are hardware topology; `traffic lower bound` is application-level bytes touched, not DRAM-controller bandwidth.\n");

    output.push_str("\n## Results\n\n");
    output.push_str("| Workload | Algorithm | Median | Min–max | Process CPU | Working-set delta | Traffic lower bound |\n");
    output.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for row in rows {
        output.push_str(&format!(
            "| {} | {} | {} | {} – {} | {} | {:+} KiB | {} |\n",
            row.workload,
            row.algorithm,
            fmt_duration(row.median),
            fmt_duration(row.min),
            fmt_duration(row.max),
            row.cpu_percent.map_or_else(
                || "N/A (< counter resolution)".to_owned(),
                |value| format!("{value:.1}%")
            ),
            row.working_set_delta / 1024,
            fmt_bytes(row.traffic_lower_bound),
        ));
    }

    output.push_str("\n## Interpretation boundaries\n\n");
    output.push_str("`HashSet`/`BTreeSet` deduplicate values, whereas BWSPI and `Vec` retain duplicates. They are included for contains/insert reference, not as semantic replacements. `Vec::binary_search` includes the one-time pre-sort only in the sort rows, not in lookup timing. Rust's `sort_unstable` and stable `sort` are the standard-library sorting baselines.\n");
    output
}

fn main() {
    let element_count = std::env::var("BWSPI_BENCH_N")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&value| value >= LOOKUPS)
        .unwrap_or(DEFAULT_ELEMENTS);
    let entropy = high_entropy(element_count);
    let uniform = uniform_width(element_count);
    let before_temperature = acpi_temperature_celsius();
    let mut rows = Vec::new();

    run_stream_workloads(&mut rows, "stream, high entropy", &entropy);
    run_stream_workloads(&mut rows, "stream, uniform 32-bit", &uniform);
    run_sort_workloads(&mut rows, "sort, high entropy", &entropy);
    run_sort_workloads(&mut rows, "sort, uniform 32-bit", &uniform);
    run_update_delete_workloads(&mut rows, &entropy);

    let after_temperature = acpi_temperature_celsius();
    let markdown = report(
        &rows,
        element_count,
        &before_temperature,
        &after_temperature,
    );
    std::fs::write("bench.md", &markdown).expect("write bench.md");
    print!("{markdown}");
}

#[cfg(windows)]
mod telemetry {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null_mut, read_unaligned};
    use std::time::Duration;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct GroupAffinity {
        mask: usize,
        group: u16,
        reserved: [u16; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CacheRelationship {
        level: u8,
        associativity: u8,
        line_size: u16,
        cache_size: u32,
        cache_type: u32,
        group_mask: GroupAffinity,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LogicalProcessorHeader {
        relationship: u32,
        size: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn GetLogicalProcessorInformationEx(
            relationship: u32,
            buffer: *mut c_void,
            returned_length: *mut u32,
        ) -> i32;
        fn GetLastError() -> u32;
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }

    fn filetime_duration(filetime: FileTime) -> Duration {
        let ticks = ((filetime.high as u64) << 32) | filetime.low as u64;
        Duration::from_nanos(ticks.saturating_mul(100))
    }

    #[derive(Clone, Copy)]
    pub struct ProcessSnapshot {
        cpu: Duration,
        working_set: usize,
    }

    impl ProcessSnapshot {
        pub fn cpu_percent_since(self, before: Self, elapsed: Duration) -> f64 {
            let cpu = self.cpu.saturating_sub(before.cpu).as_secs_f64();
            if elapsed.is_zero() {
                0.0
            } else {
                cpu / elapsed.as_secs_f64() * 100.0
            }
        }

        pub fn working_set_delta(self, before: Self) -> isize {
            self.working_set as isize - before.working_set as isize
        }
    }

    pub fn process_snapshot() -> ProcessSnapshot {
        // SAFETY: structures are initialized to their documented sizes and the
        // returned handles belong to this process.
        unsafe {
            let mut creation = FileTime::default();
            let mut exit = FileTime::default();
            let mut kernel = FileTime::default();
            let mut user = FileTime::default();
            let _ = GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            );
            let mut counters: ProcessMemoryCounters = zeroed();
            counters.cb = size_of::<ProcessMemoryCounters>() as u32;
            let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
            ProcessSnapshot {
                cpu: filetime_duration(kernel) + filetime_duration(user),
                working_set: counters.working_set_size,
            }
        }
    }

    #[derive(Clone, Copy)]
    struct CacheInfo {
        level: u8,
        size: u32,
        line_size: u16,
    }

    fn caches() -> Vec<CacheInfo> {
        const RELATION_CACHE: u32 = 2;
        const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
        // SAFETY: Windows documents the two-call buffer-size pattern. The
        // parser advances by each self-describing record size and uses
        // unaligned reads because the backing buffer is `Vec<u8>`.
        unsafe {
            let mut length = 0_u32;
            if GetLogicalProcessorInformationEx(RELATION_CACHE, null_mut(), &mut length) != 0
                || GetLastError() != ERROR_INSUFFICIENT_BUFFER
                || length == 0
            {
                return Vec::new();
            }
            let mut buffer = vec![0_u8; length as usize];
            if GetLogicalProcessorInformationEx(
                RELATION_CACHE,
                buffer.as_mut_ptr().cast::<c_void>(),
                &mut length,
            ) == 0
            {
                return Vec::new();
            }
            let mut offset = 0_usize;
            let mut result = Vec::new();
            while offset + size_of::<LogicalProcessorHeader>() <= length as usize {
                let base = buffer.as_ptr().add(offset);
                let header = read_unaligned(base.cast::<LogicalProcessorHeader>());
                if header.size as usize <= size_of::<LogicalProcessorHeader>()
                    || offset + header.size as usize > length as usize
                {
                    break;
                }
                if header.relationship == RELATION_CACHE
                    && header.size as usize
                        >= size_of::<LogicalProcessorHeader>() + size_of::<CacheRelationship>()
                {
                    let cache = read_unaligned(
                        base.add(size_of::<LogicalProcessorHeader>())
                            .cast::<CacheRelationship>(),
                    );
                    result.push(CacheInfo {
                        level: cache.level,
                        size: cache.cache_size,
                        line_size: cache.line_size,
                    });
                }
                offset += header.size as usize;
            }
            result
        }
    }

    pub fn platform_report() -> String {
        // SAFETY: MemoryStatusEx receives its correctly sized initialized C
        // structure and only writes its documented fields.
        let memory = unsafe {
            let mut status: MemoryStatusEx = zeroed();
            status.length = size_of::<MemoryStatusEx>() as u32;
            if GlobalMemoryStatusEx(&mut status) == 0 {
                None
            } else {
                Some((status.total_phys, status.avail_phys, status.memory_load))
            }
        };
        let mut output = String::new();
        if let Some((total, available, load)) = memory {
            output.push_str(&format!(
                "- Physical RAM: {:.2} GiB total, {:.2} GiB available, {load}% committed load at report time.\n",
                total as f64 / 1024_f64.powi(3),
                available as f64 / 1024_f64.powi(3),
            ));
        } else {
            output.push_str("- Physical RAM: unavailable from GlobalMemoryStatusEx.\n");
        }
        let caches = caches();
        if caches.is_empty() {
            output
                .push_str("- Cache topology: unavailable from GetLogicalProcessorInformationEx.\n");
        } else {
            for level in 1..=3_u8 {
                let entries: Vec<_> = caches.iter().filter(|cache| cache.level == level).collect();
                if !entries.is_empty() {
                    let total: u64 = entries.iter().map(|cache| cache.size as u64).sum();
                    let line = entries[0].line_size;
                    output.push_str(&format!(
                        "- L{level} cache topology: {} logical cache records, {:.2} MiB aggregate reported capacity, {line}-byte line.\n",
                        entries.len(),
                        total as f64 / (1024.0 * 1024.0),
                    ));
                }
            }
        }
        output.push_str("- Per-result CPU percent: process kernel+user time divided by wall time; it can exceed 100% for multi-core execution.\n");
        output.push_str("- Per-result working-set delta: GetProcessMemoryInfo before/after the measured operation.\n");
        output
    }
}

#[cfg(not(windows))]
mod telemetry {
    use std::time::Duration;

    #[derive(Clone, Copy)]
    pub struct ProcessSnapshot;

    impl ProcessSnapshot {
        pub fn cpu_percent_since(self, _: Self, _: Duration) -> f64 {
            0.0
        }

        pub fn working_set_delta(self, _: Self) -> isize {
            0
        }
    }

    pub fn process_snapshot() -> ProcessSnapshot {
        ProcessSnapshot
    }

    pub fn platform_report() -> String {
        "- Windows-native telemetry unavailable: this benchmark is running on a non-Windows target.\n".to_owned()
    }
}
