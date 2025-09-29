/// Output context - outputs exactly N bytes when requested
    fn spawn_output_context(&self) -> tokio::task::JoinHandle<()> {
        let transport = self.transport.clone();
        let output_buffer = self.output_buffer.clone();
        let// optimized_double_buffered_io.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex, mpsc};
use tokio::time::sleep;
use thiserror::Error;
use log::{info, warn, debug, error};

use crate::transport::Transport;
use crate::processor::DataProcessor;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("Pipeline stopped")]
    Stopped,
    #[error("Transport error: {0}")]
    TransportError(#[from] crate::Error),
}

/// Ring buffer for efficient data management
struct RingBuffer {
    data: Vec<u8>,
    capacity: usize,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
    available: AtomicUsize,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: vec![0u8; capacity],
            capacity,
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
            available: AtomicUsize::new(capacity), // Initially filled with zeros
        }
    }

    /// Write data to buffer, returns amount written
    fn write(&mut self, data: &[u8]) -> usize {
        let available_space = self.capacity - self.available.load(Ordering::Acquire);
        let to_write = data.len().min(available_space);
        
        if to_write == 0 {
            return 0;
        }

        let write_pos = self.write_pos.load(Ordering::Acquire);
        
        // Handle wrap-around
        if write_pos + to_write <= self.capacity {
            self.data[write_pos..write_pos + to_write].copy_from_slice(&data[..to_write]);
        } else {
            let first_part = self.capacity - write_pos;
            self.data[write_pos..].copy_from_slice(&data[..first_part]);
            self.data[..to_write - first_part].copy_from_slice(&data[first_part..to_write]);
        }
        
        self.write_pos.store((write_pos + to_write) % self.capacity, Ordering::Release);
        self.available.fetch_add(to_write, Ordering::AcqRel);
        to_write
    }

    /// Read data from buffer, returns data read
    fn read(&mut self, count: usize) -> Vec<u8> {
        let available = self.available.load(Ordering::Acquire);
        let to_read = count.min(available);
        
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let mut result = vec![0u8; to_read];
        
        // Handle wrap-around
        if read_pos + to_read <= self.capacity {
            result.copy_from_slice(&self.data[read_pos..read_pos + to_read]);
        } else {
            let first_part = self.capacity - read_pos;
            result[..first_part].copy_from_slice(&self.data[read_pos..]);
            result[first_part..].copy_from_slice(&self.data[..to_read - first_part]);
        }
        
        self.read_pos.store((read_pos + to_read) % self.capacity, Ordering::Release);
        self.available.fetch_sub(to_read, Ordering::AcqRel);
        result
    }

    /// Peek at data without consuming
    fn peek(&self, count: usize) -> Vec<u8> {
        let available = self.available.load(Ordering::Acquire);
        let to_read = count.min(available);
        
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let mut result = vec![0u8; to_read];
        
        if read_pos + to_read <= self.capacity {
            result.copy_from_slice(&self.data[read_pos..read_pos + to_read]);
        } else {
            let first_part = self.capacity - read_pos;
            result[..first_part].copy_from_slice(&self.data[read_pos..]);
            result[first_part..].copy_from_slice(&self.data[..to_read - first_part]);
        }
        
        result
    }

    fn available_data(&self) -> usize {
        self.available.load(Ordering::Acquire)
    }
}

/// Performance metrics
#[derive(Debug, Clone)]
pub struct PipelineMetrics {
    pub input_bytes: Arc<AtomicUsize>,
    pub output_bytes: Arc<AtomicUsize>,
    pub processed_bytes: Arc<AtomicUsize>,
    pub processing_count: Arc<AtomicUsize>,  // Add this field
    pub overflow_count: Arc<AtomicUsize>,    // Add this field
    pub underflow_count: Arc<AtomicUsize>,
    pub processing_time_ms: Arc<AtomicUsize>,
}

impl PipelineMetrics {
    fn new() -> Self {
        Self {
            input_bytes: Arc::new(AtomicUsize::new(0)),
            output_bytes: Arc::new(AtomicUsize::new(0)),
            processed_bytes: Arc::new(AtomicUsize::new(0)),
            processing_count: Arc::new(AtomicUsize::new(0)),
            overflow_count: Arc::new(AtomicUsize::new(0)),
            underflow_count: Arc::new(AtomicUsize::new(0)),
            processing_time_ms: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// Pipeline configuration
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub buffer_size: usize,
    pub max_processing_time: Duration,
    pub timeout: Duration,
    pub read_chunk_size: usize,
    #[doc(hidden)]
    pub processing_timeout: Duration,  // Internal use, not meant to be set by users
}

impl PipelineConfig {
    /// Create a new config with explicit values
    pub fn new(
        buffer_size: usize,
        max_processing_time: Duration,
        timeout: Duration,
        read_chunk_size: usize,
    ) -> Self {
        Self {
            buffer_size,
            max_processing_time,
            timeout,
            read_chunk_size,
            processing_timeout: max_processing_time,  // Automatically set
        }
    }
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            buffer_size: 8192,
            max_processing_time: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 1024,
            processing_timeout: Duration::from_secs(1),
        }
    }
}

/// Output request for immediate output
#[derive(Debug)]
struct OutputRequest {
    byte_count: usize,
}

/// Optimized double-buffered I/O pipeline
pub struct DoubleBufferedIO<T: Transport, P: DataProcessor> {
    transport: Arc<RwLock<T>>,
    processor: Arc<P>,
    output_buffer: Arc<Mutex<RingBuffer>>,
    config: PipelineConfig,
    metrics: Arc<PipelineMetrics>,
    running: Arc<AtomicBool>,
    output_tx: mpsc::UnboundedSender<OutputRequest>,
    output_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<OutputRequest>>>>,  // Wrapped in Option for taking
}

impl<T: Transport + 'static, P: DataProcessor + 'static> DoubleBufferedIO<T, P> {
    pub fn new(transport: T, processor: P, config: PipelineConfig) -> Self {
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        
        Self {
            transport: Arc::new(RwLock::new(transport)),
            processor: Arc::new(processor),
            output_buffer: Arc::new(Mutex::new(RingBuffer::new(config.buffer_size))),
            config,
            metrics: Arc::new(PipelineMetrics::new()),
            running: Arc::new(AtomicBool::new(false)),
            output_tx,
            output_rx: Arc::new(Mutex::new(output_rx)),
        }
    }

    /// Start the pipeline
    pub async fn start(&self) -> Result<(), PipelineError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already running
        }

        // Pre-fill output buffer with zeros (so initial outputs work)
        {
            let mut buffer = self.output_buffer.lock().await;
            // Buffer is already initialized with zeros and available count = capacity
        }

        // Spawn only two contexts - input and output
        let input_handle = self.spawn_input_context();
        let output_handle = self.spawn_output_context();

        info!("Pipeline started with 2 contexts");

        // Monitor handles
        tokio::spawn(async move {
            tokio::select! {
                _ = input_handle => error!("Input context stopped"),
                _ = output_handle => error!("Output context stopped"),
            }
        });

        Ok(())
    }

    /// Stop the pipeline
    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // Send a zero-byte request to wake up output thread
        let _ = self.output_tx.send(OutputRequest { byte_count: 0 });
        sleep(Duration::from_millis(100)).await;
        info!("Pipeline stopped");
    }

    /// Get current metrics
    pub fn metrics(&self) -> PipelineMetrics {
        PipelineMetrics {
            input_bytes: Arc::new(AtomicUsize::new(
                self.metrics.input_bytes.load(Ordering::Relaxed)
            )),
            output_bytes: Arc::new(AtomicUsize::new(
                self.metrics.output_bytes.load(Ordering::Relaxed)
            )),
            processed_bytes: Arc::new(AtomicUsize::new(
                self.metrics.processed_bytes.load(Ordering::Relaxed)
            )),
            processing_count: Arc::new(AtomicUsize::new(
                self.metrics.processing_count.load(Ordering::Relaxed)
            )),
            overflow_count: Arc::new(AtomicUsize::new(
                self.metrics.overflow_count.load(Ordering::Relaxed)
            )),
            underflow_count: Arc::new(AtomicUsize::new(
                self.metrics.underflow_count.load(Ordering::Relaxed)
            )),
            processing_time_ms: Arc::new(AtomicUsize::new(
                self.metrics.processing_time_ms.load(Ordering::Relaxed)
            )),
        }
    }

    /// Input context - reads data and immediately triggers output
    fn spawn_input_context(&self) -> tokio::task::JoinHandle<()> {
        let transport = self.transport.clone();
        let processor = self.processor.clone();
        let output_buffer = self.output_buffer.clone();
        let output_tx = self.output_tx.clone();
        let metrics = self.metrics.clone();
        let running = self.running.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            let mut read_buffer = vec![0u8; config.read_chunk_size];
            
            // Create a dedicated channel for processing results
            let (process_tx, mut process_rx) = mpsc::unbounded_channel();

            // Spawn a single dedicated processor task to handle all processing
            let processor_handle = {
                let processor = processor.clone();
                let output_buffer = output_buffer.clone();
                let metrics = metrics.clone();
                let running = running.clone();
                
                tokio::spawn(async move {
                    while running.load(Ordering::Relaxed) {
                        if let Some(data) = process_rx.recv().await {
                            let start = Instant::now();
                            match processor.process(data).await {
                                Ok(processed_data) => {
                                    let elapsed = start.elapsed();
                                    metrics.processing_time_ms.store(
                                        elapsed.as_millis() as usize, 
                                        Ordering::Relaxed
                                    );
                                    metrics.processing_count.fetch_add(1, Ordering::Relaxed);
                                    
                                    // Write processed data to output buffer
                                    let mut buffer = output_buffer.lock().await;
                                    let written = buffer.write(&processed_data);
                                    metrics.processed_bytes.fetch_add(written, Ordering::Relaxed);
                                    
                                    debug!("Processing: Wrote {} bytes to output buffer ({}ms)", 
                                           written, elapsed.as_millis());
                                    if written < processed_data.len() {
                                        warn!("Output buffer full, dropped {} bytes", 
                                              processed_data.len() - written);
                                        metrics.overflow_count.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => {
                                    error!("Processing error: {}", e);
                                }
                            }
                        }
                    }
                })
            };

            while running.load(Ordering::Relaxed) {
                debug!("Waiting for input...");
                let transport_guard = transport.read().await;
                debug!("Acquired transport lock");
                match transport_guard.receive(&mut read_buffer).await {
                    Ok(bytes_read) if bytes_read > 0 => {
                        drop(transport_guard);
                        let recv_time = Instant::now();
                        
                        metrics.input_bytes.fetch_add(bytes_read, Ordering::Relaxed);
                        debug!("Input: Received {} bytes at {:?}", bytes_read, recv_time);

                        // CRITICAL: Immediately request output of same size for minimum latency
                        if let Err(e) = output_tx.send(OutputRequest { byte_count: bytes_read }) {
                            error!("Failed to send output request: {}", e);
                            break;
                        }
                        let send_time = Instant::now();
                        debug!("Input: Output request sent, latency: {}µs", 
                               (send_time - recv_time).as_micros());

                        // Queue data for processing (non-blocking)
                        let data_to_process = read_buffer[..bytes_read].to_vec();
                        if let Err(e) = process_tx.send(data_to_process) {
                            error!("Failed to queue for processing: {}", e);
                        }
                        
                        // Immediately continue to next input without waiting
                    }
                    Ok(_) => {
                        drop(transport_guard);
                        // Use yield_now instead of sleep for better responsiveness
                        tokio::task::yield_now().await;
                    }
                    Err(e) => {
                        if running.load(Ordering::Relaxed) {
                            warn!("Transport receive error: {}", e);
                        }
                        drop(transport_guard);
                        sleep(Duration::from_millis(1)).await;
                    }
                }
            }
            
            // Clean up processor task
            drop(process_tx);
            let _ = processor_handle.await;
            debug!("Input context stopped");
        })
    }

    /// Output context - outputs exactly N bytes when requested
    fn spawn_output_context(&self) -> tokio::task::JoinHandle<()> {
        let transport = self.transport.clone();
        let output_buffer = self.output_buffer.clone();
        let output_rx_arc = self.output_rx.clone();
        let metrics = self.metrics.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            // Take ownership of the receiver
            let mut output_rx = {
                let mut rx_option = output_rx_arc.lock().await;
                rx_option.take().expect("Output receiver already taken")
            };
            
            let mut request_count = 0u64;
            
            while running.load(Ordering::Relaxed) {
                match output_rx.recv().await {
                    Some(request) => {
                        if request.byte_count == 0 {
                            break; // Shutdown signal
                        }
                        
                        request_count += 1;
                        let start_time = Instant::now();
                        debug!("Output[{}]: Request to output {} bytes at {:?}", 
                               request_count, request.byte_count, start_time);

                        // Get data from output buffer or use zeros
                        let output_data = {
                            let mut buffer = output_buffer.lock().await;
                            let available = buffer.available_data();
                            
                            if available >= request.byte_count {
                                // We have enough processed data
                                buffer.read(request.byte_count)
                            } else {
                                // Not enough data - use what we have + zeros
                                let mut data = buffer.read(available);
                                data.resize(request.byte_count, 0);
                                
                                if request.byte_count > available {
                                    metrics.underflow_count.fetch_add(1, Ordering::Relaxed);
                                    debug!("Output[{}]: Underflow - had {} bytes, padded {} with zeros", 
                                           request_count, available, request.byte_count - available);
                                }
                                data
                            }
                        };

                        // Send exactly the requested bytes IMMEDIATELY
                        assert_eq!(output_data.len(), request.byte_count);
                        
                        let transport_guard = transport.read().await;
                        let send_start = Instant::now();
                        match transport_guard.send(&output_data).await {
                            Ok(()) => {
                                drop(transport_guard);
                                metrics.output_bytes.fetch_add(output_data.len(), Ordering::Relaxed);
                                let total_latency = send_start.duration_since(start_time);
                                debug!("Output[{}]: Sent {} bytes with {}µs total latency", 
                                       request_count, output_data.len(), total_latency.as_micros());
                            }
                            Err(e) => {
                                drop(transport_guard);
                                error!("Transport send error: {}", e);
                                break;
                            }
                        }
                    }
                    None => {
                        debug!("Output channel closed");
                        break;
                    }
                }
            }
            debug!("Output context stopped after {} requests", request_count);
        })
    }
}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processor::PassThroughProcessor;
    use async_trait::async_trait;
    use std::collections::VecDeque;

    // Mock transport for testing
    #[derive(Clone)]
    struct MockTransport {
        sent_data: Arc<Mutex<Vec<u8>>>,
        receive_data: Arc<Mutex<VecDeque<u8>>>,
    }

    impl MockTransport {
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
    impl Transport for MockTransport {
        async fn send(&self, data: &[u8]) -> Result<(), crate::Error> {
            let mut sent = self.sent_data.lock().await;
            sent.extend_from_slice(data);
            Ok(())
        }

        async fn receive(&self, buffer: &mut [u8]) -> Result<usize, crate::Error> {
            let mut queue = self.receive_data.lock().await;
            let to_read = buffer.len().min(queue.len());
            for i in 0..to_read {
                buffer[i] = queue.pop_front().unwrap();
            }
            Ok(to_read)
        }

        async fn receive_from(&self, buffer: &mut [u8]) -> Result<(usize, std::net::SocketAddr), crate::Error> {
            let size = self.receive(buffer).await?;
            Ok((size, "127.0.0.1:8080".parse().unwrap()))
        }

        async fn send_to(&self, data: &[u8], _addr: std::net::SocketAddr) -> Result<(), crate::Error> {
            self.send(data).await
        }

        fn set_timeout(&mut self, _timeout: Duration) {}
    }

    #[tokio::test]
    async fn test_ring_buffer() {
        let mut buffer = RingBuffer::new(10);
        
        // Test write
        let written = buffer.write(&[1, 2, 3, 4, 5]);
        assert_eq!(written, 5);
        assert_eq!(buffer.available_data(), 10); // 5 new + 5 zeros
        
        // Test read
        let data = buffer.read(3);
        assert_eq!(data, vec![0, 0, 0]); // Should read zeros first
        assert_eq!(buffer.available_data(), 7);
        
        // Test wrap-around
        buffer.write(&[6, 7, 8]);
        assert_eq!(buffer.available_data(), 10);
    }

    #[tokio::test]
    async fn test_exact_byte_correspondence() {
        env_logger::init();
        
        let transport = MockTransport::new();
        let processor = PassThroughProcessor::new(Duration::from_millis(1));
        
        let config = PipelineConfig {
            buffer_size: 1024,
            max_processing_time: Duration::from_secs(1),
            processing_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 128,
        };
        
        let pipeline = DoubleBufferedIO::new(transport.clone(), processor, config);
        pipeline.start().await.unwrap();
        
        // Test various input sizes
        let test_cases = vec![
            vec![1, 2, 3, 4, 5],           // 5 bytes
            vec![6; 10],                     // 10 bytes
            vec![7; 100],                    // 100 bytes
            vec![8, 9],                      // 2 bytes
        ];
        
        let mut total_input = 0;
        for test_data in test_cases {
            total_input += test_data.len();
            transport.add_input_data(&test_data).await;
            sleep(Duration::from_millis(50)).await;
        }
        
        sleep(Duration::from_millis(200)).await;
        
        // Verify exact correspondence
        let metrics = pipeline.metrics();
        let input_bytes = metrics.input_bytes.load(Ordering::Relaxed);
        let output_bytes = metrics.output_bytes.load(Ordering::Relaxed);
        
        assert_eq!(input_bytes, total_input);
        assert_eq!(output_bytes, input_bytes, "Output must equal input");
        
        let sent_data = transport.get_sent_data().await;
        assert_eq!(sent_data.len(), total_input, "Total sent must equal input");
        
        pipeline.stop().await;
    }

    #[tokio::test]
    async fn test_streaming_consistency() {
        let transport = MockTransport::new();
        let processor = PassThroughProcessor::new(Duration::from_millis(1));
        
        let config = PipelineConfig {
            buffer_size: 256,
            max_processing_time: Duration::from_secs(1),
            processing_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 32,
        };
        
        let pipeline = DoubleBufferedIO::new(transport.clone(), processor, config);
        pipeline.start().await.unwrap();
        
        // Stream data continuously
        for i in 0..50 {
            let data = vec![i as u8; 3];
            transport.add_input_data(&data).await;
            sleep(Duration::from_millis(10)).await;
        }
        
        sleep(Duration::from_millis(500)).await;
        
        let metrics = pipeline.metrics();
        assert_eq!(
            metrics.input_bytes.load(Ordering::Relaxed),
            metrics.output_bytes.load(Ordering::Relaxed),
            "Streaming must maintain byte correspondence"
        );
        
        pipeline.stop().await;
    }
}