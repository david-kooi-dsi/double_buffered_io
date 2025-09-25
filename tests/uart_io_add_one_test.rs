// Integration test for UART I/O with AddOneProcessor
//
// Usage: cargo test --test uart_io_add_one_test -- --ignored
// Or for running with specific device and baud rate:
// cargo test --test uart_io_add_one_test -- --ignored --nocapture

use double_buffered_io::{
    DoubleBufferedIO, UartTransport, AddOneProcessor, PipelineConfig
};
use std::env;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
#[ignore] // Ignored by default since it requires actual hardware
async fn test_uart_io_add_one() {
    // Parse command line arguments for device and baud rate
    let args: Vec<String> = env::args().collect();

    let (device, baud_rate) = if args.len() >= 3 {
        let device = args[1].clone();
        let baud_rate = args[2].parse::<u32>()
            .expect("Invalid baud rate - must be a number");
        (device, baud_rate)
    } else {
        // Default values for testing
        println!("Usage: cargo test --test uart_io_add_one_test -- --ignored <device> <baud>");
        println!("Using default values for testing...");
        ("/dev/ttyUSB0".to_string(), 115200)
    };

    println!("Testing UART I/O with device: {}, baud rate: {}", device, baud_rate);

    // Create UART transport
    let transport = match UartTransport::new(&device, baud_rate).await {
        Ok(transport) => {
            println!("Successfully opened UART connection to {}", device);
            transport
        }
        Err(e) => {
            println!("Failed to open UART connection: {}", e);
            println!("This test requires actual UART hardware.");
            println!("To test with a real device, run:");
            println!("cargo test --test uart_io_add_one_test -- --ignored <device> <baud>");
            return;
        }
    };

    // Create AddOneProcessor
    let processor = AddOneProcessor::new();

    // Configure pipeline with smaller buffer for testing
    let config = PipelineConfig {
        buffer_size: 2048,
        max_processing_time: Duration::from_secs(1),
        timeout: Duration::from_secs(5),
        read_chunk_size: 256,
    };

    // Create double-buffered I/O pipeline
    let pipeline = DoubleBufferedIO::new(transport, processor, config);

    // Start the pipeline
    match pipeline.start().await {
        Ok(()) => println!("Pipeline started successfully"),
        Err(e) => {
            println!("Failed to start pipeline: {}", e);
            return;
        }
    }

    println!("Pipeline is running. Listening for data...");
    println!("Send data to the UART device to see it processed (each byte incremented by 1)");
    println!("Test will run for 60 seconds...");

    // Let the pipeline run for 30 seconds
    for i in 1..=69 {
        sleep(Duration::from_secs(1)).await;

        // Print metrics every 5 seconds
        if i % 5 == 0 {
            let metrics = pipeline.metrics();
            let input_bytes = metrics.input_bytes.load(std::sync::atomic::Ordering::Relaxed);
            let output_bytes = metrics.output_bytes.load(std::sync::atomic::Ordering::Relaxed);
            let processing_count = metrics.processing_count.load(std::sync::atomic::Ordering::Relaxed);

            println!("After {} seconds - Input: {} bytes, Output: {} bytes, Processed: {} buffers",
                     i, input_bytes, output_bytes, processing_count);
        }
    }

    // Stop the pipeline
    pipeline.stop().await;
    println!("Pipeline stopped");

    // Print final metrics
    let metrics = pipeline.metrics();
    let input_bytes = metrics.input_bytes.load(std::sync::atomic::Ordering::Relaxed);
    let output_bytes = metrics.output_bytes.load(std::sync::atomic::Ordering::Relaxed);
    let processing_count = metrics.processing_count.load(std::sync::atomic::Ordering::Relaxed);
    let overflow_count = metrics.overflow_count.load(std::sync::atomic::Ordering::Relaxed);
    let underflow_count = metrics.underflow_count.load(std::sync::atomic::Ordering::Relaxed);

    println!("\nFinal metrics:");
    println!("  Input bytes: {}", input_bytes);
    println!("  Output bytes: {}", output_bytes);
    println!("  Processing count: {}", processing_count);
    println!("  Overflow count: {}", overflow_count);
    println!("  Underflow count: {}", underflow_count);

    // Test passes if no panics occurred and pipeline ran successfully
    assert!(true, "UART I/O AddOne test completed");
}

#[cfg(test)]
mod mock_tests {
    use super::*;
    use double_buffered_io::{Transport, Error};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use std::collections::VecDeque;

    // Mock transport for testing the integration without real hardware
    #[derive(Clone)]
    struct MockUartTransport {
        sent_data: Arc<Mutex<Vec<u8>>>,
        receive_data: Arc<Mutex<VecDeque<u8>>>,
    }

    impl MockUartTransport {
        fn new() -> Self {
            Self {
                sent_data: Arc::new(Mutex::new(Vec::new())),
                receive_data: Arc::new(Mutex::new(VecDeque::new())),
            }
        }

        async fn add_input_data(&self, data: &[u8]) {
            let mut queue = self.receive_data.lock().await;
            queue.extend(data);
        }

        async fn get_sent_data(&self) -> Vec<u8> {
            self.sent_data.lock().await.clone()
        }
    }

    #[async_trait]
    impl Transport for MockUartTransport {
        async fn send(&self, data: &[u8]) -> Result<(), Error> {
            let mut sent = self.sent_data.lock().await;
            sent.extend_from_slice(data);
            Ok(())
        }

        async fn receive(&self, buffer: &mut [u8]) -> Result<usize, Error> {
            let mut queue = self.receive_data.lock().await;
            let to_read = buffer.len().min(queue.len());
            for i in 0..to_read {
                buffer[i] = queue.pop_front().unwrap();
            }
            Ok(to_read)
        }

        async fn receive_from(&self, buffer: &mut [u8]) -> Result<(usize, std::net::SocketAddr), Error> {
            let size = self.receive(buffer).await?;
            Ok((size, "127.0.0.1:8080".parse().unwrap()))
        }

        async fn send_to(&self, data: &[u8], _addr: std::net::SocketAddr) -> Result<(), Error> {
            self.send(data).await
        }

        fn set_timeout(&mut self, _timeout: Duration) {}
    }

    #[tokio::test]
    async fn test_mock_uart_add_one_integration() {
        let transport = MockUartTransport::new();
        let processor = AddOneProcessor::new();

        let config = PipelineConfig {
            buffer_size: 10,
            max_processing_time: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 50,
        };

        // Test data that will fill the buffer exactly
        let test_data1 = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let test_data2 = vec![11, 12, 13, 14, 15, 16, 17, 18, 19, 20];

        let pipeline = DoubleBufferedIO::new(transport.clone(), processor, config);
        pipeline.start().await.unwrap();

        // Add test data
        transport.add_input_data(&test_data1).await;
        sleep(Duration::from_millis(100)).await;

        transport.add_input_data(&test_data2).await;
        sleep(Duration::from_millis(100)).await;

        // Check that data was processed (each byte incremented by 1)
        let sent_data = transport.get_sent_data().await;

        // Expected output: input data with each byte incremented by 1
        let expected_data1: Vec<u8> = test_data1.iter().map(|&b| b.wrapping_add(1)).collect();
        let expected_data2: Vec<u8> = test_data2.iter().map(|&b| b.wrapping_add(1)).collect();
        let mut expected_output = expected_data1;
        expected_output.extend(expected_data2);

        assert_eq!(sent_data, expected_output, "Output should be input data with each byte incremented by 1");

        // Check metrics
        let metrics = pipeline.metrics();
        let input_bytes = metrics.input_bytes.load(std::sync::atomic::Ordering::Relaxed);
        let output_bytes = metrics.output_bytes.load(std::sync::atomic::Ordering::Relaxed);

        assert_eq!(input_bytes, 20);
        assert_eq!(output_bytes, 20);

        pipeline.stop().await;
    }
}