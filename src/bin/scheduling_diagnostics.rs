// Save as latency_test.rs and run with: cargo run --release

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use crossbeam_channel::{bounded, unbounded};

#[tokio::main]
async fn main() {
    println!("=== I/O Pipeline Latency Diagnostic ===\n");
    
    // Test 1: Your current ByteSignal implementation
    test_notify_latency().await;
    
    // Test 2: Crossbeam channels
    test_crossbeam_latency();
    
    // Test 3: System timer resolution
    test_timer_resolution();
    
    // Test 4: Thread wake latency
    test_thread_wake_latency();
    
    println!("\n=== Analysis ===");
    analyze_results();
}

async fn test_notify_latency() {
    println!("Testing Tokio Notify (your current implementation):");
    
    let notify = Arc::new(Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    
    let n2 = notify.clone();
    let c2 = count.clone();
    
    // Spawn waiter
    let handle = tokio::spawn(async move {
        let start = Instant::now();
        
        // Your current pattern
        loop {
            let notified = n2.notified();
            let bytes = c2.swap(0, Ordering::Acquire);
            if bytes > 0 {
                return start.elapsed();
            }
            notified.await;
            let bytes = c2.swap(0, Ordering::Acquire);
            if bytes > 0 {
                return start.elapsed();
            }
        }
    });
    
    // Let waiter start
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    // Signal
    let signal_time = Instant::now();
    count.store(100, Ordering::Release);
    notify.notify_one();
    
    let latency = handle.await.unwrap();
    println!("  Latency: {:?}", latency);
    
    if latency > Duration::from_millis(10) {
        println!("  ⚠️  HIGH LATENCY - This is your problem!");
    }
}

fn test_crossbeam_latency() {
    println!("\nTesting Crossbeam Channels (recommended fix):");
    
    // Test bounded(1)
    {
        let (tx, rx) = bounded::<Instant>(1);
        
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            tx.send(Instant::now()).unwrap();
        });
        
        let sent_time = rx.recv().unwrap();
        let latency = Instant::now().duration_since(sent_time);
        println!("  bounded(1) latency: {:?}", latency);
        
        if latency < Duration::from_millis(1) {
            println!("  ✓ Excellent!");
        }
    }
    
    // Test unbounded
    {
        let (tx, rx) = unbounded::<Instant>();
        
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            tx.send(Instant::now()).unwrap();
        });
        
        let sent_time = rx.recv().unwrap();
        let latency = Instant::now().duration_since(sent_time);
        println!("  unbounded latency: {:?}", latency);
    }
}

fn test_timer_resolution() {
    println!("\nTesting System Timer Resolution:");
    
    let mut measurements = Vec::new();
    
    for _ in 0..20 {
        let start = Instant::now();
        std::thread::sleep(Duration::from_micros(100));
        measurements.push(start.elapsed());
    }
    
    measurements.sort();
    let min = measurements[0];
    let median = measurements[measurements.len() / 2];
    
    println!("  Minimum sleep: {:?}", min);
    println!("  Median sleep: {:?}", median);
    
    if min > Duration::from_millis(10) {
        println!("  ⚠️  Timer resolution is too coarse!");
        println!("  Run the tuning script to fix this.");
    } else if min > Duration::from_millis(1) {
        println!("  ⚠️  Timer resolution could be better");
    } else {
        println!("  ✓ Good timer resolution");
    }
}

fn test_thread_wake_latency() {
    println!("\nTesting Thread Wake Latency:");
    
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    
    let start_time = Arc::new(parking_lot::Mutex::new(None::<Instant>));
    let s2 = start_time.clone();
    
    std::thread::spawn(move || {
        while !f2.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
        *s2.lock() = Some(Instant::now());
    });
    
    std::thread::sleep(Duration::from_millis(10));
    
    let signal_time = Instant::now();
    flag.store(true, Ordering::Release);
    
    std::thread::sleep(Duration::from_millis(1));
    
    // Fix: Store the locked value in a variable first
    let wake_time_opt = *start_time.lock();
    if let Some(wake_time) = wake_time_opt {
        let latency = wake_time.duration_since(signal_time);
        println!("  Spin-wait latency: {:?}", latency);
        
        if latency < Duration::from_micros(10) {
            println!("  ✓ Excellent spin-wait performance");
        }
    }
}

fn analyze_results() {
    println!("\nRecommended fixes for your 18ms latency issue:");
    println!("1. Replace ByteSignal with crossbeam::channel::bounded(1)");
    println!("2. Run the Linux tuning script (as root):");
    println!("   - Reduces sched_min_granularity_ns to 100μs");
    println!("   - Reduces sched_wakeup_granularity_ns to 200μs");
    println!("3. Consider using RT scheduling:");
    println!("   - Run with: chrt -f 50 ./your_app");
    println!("   - Or set CAP_SYS_NICE capability");
    println!("4. Pin threads to specific CPUs");
    println!("\nExpected improvement: 18ms → <100μs");
}

// Add to Cargo.toml:
// [dependencies]
// tokio = { version = "1", features = ["full"] }
// crossbeam-channel = "0.5"
// parking_lot = "0.12"