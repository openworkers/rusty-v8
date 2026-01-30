// Benchmark comparing std::sync::Mutex vs parking_lot::Mutex
// Run with: cargo run --release --bin mutex_bench

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const ITERATIONS: u64 = 10_000_000;
const THREAD_COUNTS: &[usize] = &[2, 4, 8, 16];

fn bench_std_mutex_uncontended() -> Duration {
    let mutex = std::sync::Mutex::new(0u64);
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let mut guard = mutex.lock().unwrap();
        *guard += 1;
    }
    start.elapsed()
}

fn bench_parking_lot_uncontended() -> Duration {
    let mutex = parking_lot::Mutex::new(0u64);
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let mut guard = mutex.lock();
        *guard += 1;
    }
    start.elapsed()
}

fn bench_std_mutex_contended(threads: usize) -> Duration {
    let mutex = Arc::new(std::sync::Mutex::new(0u64));
    let start = Instant::now();

    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let mutex = Arc::clone(&mutex);
            thread::spawn(move || {
                for _ in 0..(ITERATIONS / threads as u64) {
                    let mut guard = mutex.lock().unwrap();
                    *guard += 1;
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
    start.elapsed()
}

fn bench_parking_lot_contended(threads: usize) -> Duration {
    let mutex = Arc::new(parking_lot::Mutex::new(0u64));
    let start = Instant::now();

    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let mutex = Arc::clone(&mutex);
            thread::spawn(move || {
                for _ in 0..(ITERATIONS / threads as u64) {
                    let mut guard = mutex.lock();
                    *guard += 1;
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
    start.elapsed()
}

fn format_results(name: &str, std_time: Duration, pl_time: Duration) {
    let std_ns = std_time.as_nanos() as f64 / ITERATIONS as f64;
    let pl_ns = pl_time.as_nanos() as f64 / ITERATIONS as f64;
    let speedup = std_time.as_nanos() as f64 / pl_time.as_nanos() as f64;

    println!("\n{name}:");
    println!("  std::sync::Mutex:    {std_ns:.2} ns/op");
    println!("  parking_lot::Mutex:  {pl_ns:.2} ns/op");
    println!("  Speedup:             {speedup:.2}x");
}

fn main() {
    println!("Mutex Benchmark Comparison");
    println!("==========================");
    println!("Iterations: {ITERATIONS}");

    // Size comparison (important: empty mutex size)
    println!("\nSize comparison:");
    println!("  std::sync::Mutex<()>:    {} bytes", std::mem::size_of::<std::sync::Mutex<()>>());
    println!("  parking_lot::Mutex<()>:  {} bytes", std::mem::size_of::<parking_lot::Mutex<()>>());
    println!("  std::sync::Mutex<u64>:   {} bytes", std::mem::size_of::<std::sync::Mutex<u64>>());
    println!("  parking_lot::Mutex<u64>: {} bytes", std::mem::size_of::<parking_lot::Mutex<u64>>());

    // Warmup
    let _ = bench_std_mutex_uncontended();
    let _ = bench_parking_lot_uncontended();

    // Uncontended benchmarks (run 3 times, take best)
    println!("\nRunning uncontended benchmarks (best of 3)...");
    let std_uncontended = (0..3).map(|_| bench_std_mutex_uncontended()).min().unwrap();
    let pl_uncontended = (0..3).map(|_| bench_parking_lot_uncontended()).min().unwrap();
    format_results("Uncontended (single thread)", std_uncontended, pl_uncontended);

    // Contended benchmarks with different thread counts
    println!("\nRunning contended benchmarks...");
    for &threads in THREAD_COUNTS {
        let std_contended = bench_std_mutex_contended(threads);
        let pl_contended = bench_parking_lot_contended(threads);
        format_results(&format!("Contended ({threads} threads)"), std_contended, pl_contended);
    }

    println!("\n=== Summary ===");
    println!("parking_lot::Mutex advantages:");
    println!("  - No poisoning (panic safety - if one thread panics, others continue)");
    println!("  - Cleaner API (no .unwrap() needed after .lock())");
    println!("  - 8x smaller for Mutex<()>: 1 byte vs 8 bytes");
    println!("  - Better performance under low-to-moderate contention (2-4 threads)");
    println!();
    println!("Note: std::sync::Mutex may perform better under very high contention");
    println!("      (8+ threads), but rusty_v8 usage patterns are low contention.");
}
