// Standalone binary for UART I/O with AddOneProcessor
//
// Usage: cargo run --bin uart_io_add_one <device> <baud>
// Example: cargo run --bin uart_io_add_one /dev/ttyUSB0 115200

use double_buffered_io::transport::UartTransportFixedInput;
use double_buffered_io::{
    DoubleBufferedIO, UartTransport, AddOneProcessor, PipelineConfig
};
use std::env;
use std::time::Duration;
use tokio::time::sleep;
use env_logger;


#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    ).init();
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();

    if args.len() != 3 {
        eprintln!("Usage: {} <device> <baud>", args[0]);
        eprintln!("Example: {} /dev/ttyUSB0 115200", args[0]);
        std::process::exit(1);
    }

    let device = &args[1];
    let baud_rate = match args[2].parse::<u32>() {
        Ok(baud) => baud,
        Err(_) => {
            eprintln!("Error: Invalid baud rate '{}' - must be a number", args[2]);
            std::process::exit(1);
        }
    };

    println!("UART Double-Buffered I/O with AddOneProcessor");
    println!("Device: {}", device);
    println!("Baud rate: {}", baud_rate);
    println!();

    // Create UART transport
    let fixed_input_size = 160;
    let transport = match UartTransportFixedInput::new(device, baud_rate, fixed_input_size).await {
        Ok(transport) => {
            println!("✓ Successfully opened UART connection to {}", device);
            transport
        }
        Err(e) => {
            eprintln!("✗ Failed to open UART connection: {}", e);
            eprintln!();
            eprintln!("Make sure:");
            eprintln!("  - The device exists and you have permission to access it");
            eprintln!("  - No other program is using the device");
            eprintln!("  - The device is connected and powered on");
            std::process::exit(1);
        }
    };

    // Create AddOneProcessor
    let processor = AddOneProcessor::new();

    // Configure pipeline
    let config = PipelineConfig {
        buffer_size: 2048,
        max_processing_time: Duration::from_secs(1),
        timeout: Duration::from_secs(5),
        read_chunk_size: fixed_input_size,
    };

    // Create double-buffered I/O pipeline
    let pipeline = DoubleBufferedIO::new(transport, processor, config);

    // Start the pipeline
    match pipeline.start().await {
        Ok(()) => println!("✓ Pipeline started successfully"),
        Err(e) => {
            eprintln!("✗ Failed to start pipeline: {}", e);
            std::process::exit(1);
        }
    }

    println!();
    println!("🔄 Pipeline is running...");
    println!("📡 Listening for data on {}", device);
    println!("🔢 Each received byte will be incremented by 1 and sent back");
    println!("📊 Metrics will be displayed every 10 seconds");
    println!("⏹️  Press Ctrl+C to stop");
    println!();

    // Set up Ctrl+C handler
    let pipeline_for_shutdown = std::sync::Arc::new(pipeline);
    let pipeline_clone = pipeline_for_shutdown.clone();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        println!("\n🛑 Shutting down pipeline...");
        pipeline_clone.stop().await;
        std::process::exit(0);
    });

    let mut seconds_elapsed = 0;
    loop {
        sleep(Duration::from_secs(1)).await;
        seconds_elapsed += 1;

        // Print metrics every 10 seconds
        if seconds_elapsed % 10 == 0 {
            let metrics = pipeline_for_shutdown.metrics();
            let input_bytes = metrics.input_bytes.load(std::sync::atomic::Ordering::Relaxed);
            let output_bytes = metrics.output_bytes.load(std::sync::atomic::Ordering::Relaxed);
            let processing_count = metrics.processing_count.load(std::sync::atomic::Ordering::Relaxed);
            let overflow_count = metrics.overflow_count.load(std::sync::atomic::Ordering::Relaxed);
            let underflow_count = metrics.underflow_count.load(std::sync::atomic::Ordering::Relaxed);

            println!("📊 After {} seconds:", seconds_elapsed);
            println!("   📥 Input bytes: {}", input_bytes);
            println!("   📤 Output bytes: {}", output_bytes);
            println!("   ⚙️  Processed buffers: {}", processing_count);
            if overflow_count > 0 || underflow_count > 0 {
                println!("   ⚠️  Overflows: {}, Underflows: {}", overflow_count, underflow_count);
            }
            println!();
        }
    }
}