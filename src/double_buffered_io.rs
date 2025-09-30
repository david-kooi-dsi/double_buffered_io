// double_buffered_io.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, Notify, RwLock}; // Rename tokio's Mutex
use std::sync::Mutex; // Add std::sync::Mutex for use with Crossbeam
use tokio::time::sleep;
use std::collections::VecDeque;
use thiserror::Error;
use log::{info, warn, debug, error};
use crossbeam_channel::{bounded, Sender, Receiver}; // Add crossbeam imports


// Re-export or define the Transport trait here
use crate::transport::Transport;
use crate::processor::DataProcessor;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("Input buffer overflow")]
    InputOverflow,
    #[error("Output buffer underflow")]
    OutputUnderflow,
    #[error("Processing timeout exceeded")]
    ProcessingTimeout,
    #[error("Buffer swap failed: {0}")]
    BufferSwapFailure(String),
    #[error("Transport error: {0}")]
    TransportError(#[from] crate::Error),
    #[error("Pipeline stopped")]
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferState {
    Filling,
    Full,
    Processing,
    Draining,
    Empty,
}


/// Buffer with state tracking
struct Buffer {
    data: Vec<u8>,
    state: BufferState,
    fill_level: usize,
    capacity: usize,
}

impl Buffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: vec![0u8; capacity],
            state: BufferState::Empty,
            fill_level: 0,
            capacity,
        }
    }

    fn clear(&mut self) {
        self.fill_level = 0;
        self.state = BufferState::Empty;
    }

    fn is_full(&self) -> bool {
        self.fill_level >= self.capacity
    }

    fn is_empty(&self) -> bool {
        self.fill_level == 0
    }

    fn available_space(&self) -> usize {
        self.capacity.saturating_sub(self.fill_level)
    }

    fn write(&mut self, data: &[u8]) -> usize {
        let to_write = data.len().min(self.available_space());
        if to_write > 0 {
            self.data[self.fill_level..self.fill_level + to_write]
                .copy_from_slice(&data[..to_write]);
            self.fill_level += to_write;
            if self.is_full() {
                self.state = BufferState::Full;
            } else {
                self.state = BufferState::Filling;
            }
        }
        to_write
    }

    fn read(&mut self, count: usize) -> Vec<u8> {
        let to_read = count.min(self.fill_level);
        let result = self.data[..to_read].to_vec();
        
        // Shift remaining data to the beginning
        if to_read < self.fill_level {
            self.data.copy_within(to_read..self.fill_level, 0);
        }
        
        self.fill_level -= to_read;
        if self.is_empty() {
            self.state = BufferState::Empty;
        } else {
            self.state = BufferState::Draining;
        }
        
        result
    }

    fn get_data(&self) -> Vec<u8> {
        self.data[..self.fill_level].to_vec()
    }
}

/// Double buffer implementation
struct DoubleBuffer {
    buffers: [Arc<AsyncMutex<Buffer>>; 2],
    active_index: AtomicUsize,
    swap_notify: Arc<Notify>,
}

impl DoubleBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buffers: [
                Arc::new(AsyncMutex::new(Buffer::new(capacity))),
                Arc::new(AsyncMutex::new(Buffer::new(capacity))),
            ],
            active_index: AtomicUsize::new(0),
            swap_notify: Arc::new(Notify::new()),
        }
    }

    async fn get_active(&self) -> Arc<AsyncMutex<Buffer>> {
        let index = self.active_index.load(Ordering::Acquire);
        self.buffers[index].clone()
    }

    async fn get_standby(&self) -> Arc<AsyncMutex<Buffer>> {
        let index = self.active_index.load(Ordering::Acquire);
        self.buffers[1 - index].clone()
    }
    fn get_active_index(&self) -> usize{
        self.active_index.load(Ordering::Relaxed)
    }

    async fn swap(&self) -> Result<(), PipelineError> {
        // Atomic swap of active buffer index
        let current = self.active_index.load(Ordering::Acquire);
        let new_index = 1 - current;
        
        // Verify standby buffer is ready
        let standby = self.buffers[new_index].lock().await;
        match standby.state {
            BufferState::Empty | BufferState::Processing => {
                drop(standby);
                self.active_index.store(new_index, Ordering::Release);
                self.swap_notify.notify_waiters();
                Ok(())
            }
            _ => Err(PipelineError::BufferSwapFailure(
                format!("Standby buffer in unexpected state: {:?}", standby.state)
            ))
        }
    }

    async fn wait_swap(&self) {
        self.swap_notify.notified().await;
    }
}


#[derive(Clone)]
struct ByteSignal {
    sender: Sender<usize>,
    receiver: Arc<Receiver<usize>>, // No Mutex needed - Receiver is already thread-safe
}

impl ByteSignal {
    fn new() -> Self {
        // Use bounded(0) for rendezvous channel - lowest possible latency
        // This makes sender block until receiver is ready (tight coupling!)
        let (sender, receiver) = bounded(0);
        
        Self {
            sender,
            receiver: Arc::new(receiver),
        }
    }

    fn signal_bytes(&self, bytes: usize, caller: usize) {
        let now = Instant::now();
        

        if let Err(e) = self.sender.send(bytes) {
            error!("ByteSignal: Failed to send {}: {}", bytes, e);
        } else {
            debug!("ByteSignal[{}]: Signaled {} bytes at {:?}", caller, bytes, now);
        }
    }

    // Don't use async - call this from spawn_blocking context
    fn wait_bytes_blocking(&self) -> usize {
        match self.receiver.recv() {
            Ok(bytes) => {
                let signal_received = Instant::now();
                debug!("ByteSignal: Received {} bytes at {:?}", bytes, signal_received);
                bytes
            }
            Err(e) => {
                error!("ByteSignal: Receive error: {}", e);
                0
            }
        }
    }
}

/// Performance metrics tracking
#[derive(Debug)]
pub struct PipelineMetrics {
    pub input_bytes: AtomicUsize,
    pub output_bytes: AtomicUsize,
    pub processing_count: AtomicUsize,
    pub overflow_count: AtomicUsize,
    pub underflow_count: AtomicUsize,
    pub average_processing_time_ms: AtomicUsize,
    pub buffer_utilization_percent: AtomicUsize,
}

impl Clone for PipelineMetrics {
    fn clone(&self) -> Self {
        Self {
            input_bytes: AtomicUsize::new(self.input_bytes.load(Ordering::Relaxed)),
            output_bytes: AtomicUsize::new(self.output_bytes.load(Ordering::Relaxed)),
            processing_count: AtomicUsize::new(self.processing_count.load(Ordering::Relaxed)),
            overflow_count: AtomicUsize::new(self.overflow_count.load(Ordering::Relaxed)),
            underflow_count: AtomicUsize::new(self.underflow_count.load(Ordering::Relaxed)),
            average_processing_time_ms: AtomicUsize::new(self.average_processing_time_ms.load(Ordering::Relaxed)),
            buffer_utilization_percent: AtomicUsize::new(self.buffer_utilization_percent.load(Ordering::Relaxed)),
        }
    }
}

impl PipelineMetrics {
    fn new() -> Self {
        Self {
            input_bytes: AtomicUsize::new(0),
            output_bytes: AtomicUsize::new(0),
            processing_count: AtomicUsize::new(0),
            overflow_count: AtomicUsize::new(0),
            underflow_count: AtomicUsize::new(0),
            average_processing_time_ms: AtomicUsize::new(0),
            buffer_utilization_percent: AtomicUsize::new(0),
        }
    }
}

/// Configuration for the double-buffered pipeline
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub buffer_size: usize,
    pub max_processing_time: Duration,
    pub timeout: Duration,
    pub read_chunk_size: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            buffer_size: 8192,
            max_processing_time: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 1024,
        }
    }
}

/// Main double-buffered I/O structure
pub struct DoubleBufferedIO<T: Transport, P: DataProcessor> {
    transport: Arc<RwLock<T>>,
    processor: Arc<P>,
    input_buffers: Arc<DoubleBuffer>,
    output_buffers: Arc<DoubleBuffer>,
    config: PipelineConfig,
    metrics: Arc<PipelineMetrics>,
    running: Arc<AtomicBool>,
    processing_queue: Arc<AsyncMutex<VecDeque<Vec<u8>>>>,
    output_queue: Arc<AsyncMutex<VecDeque<Vec<u8>>>>,
    // Signal for byte count coordination
    byte_signal: ByteSignal,
}

impl<T: Transport + 'static, P: DataProcessor + 'static> DoubleBufferedIO<T, P> {
    /// Create a new double-buffered I/O pipeline
    pub fn new(transport: T, processor: P, config: PipelineConfig) -> Self {
        Self {
            transport: Arc::new(RwLock::new(transport)),
            processor: Arc::new(processor),
            input_buffers: Arc::new(DoubleBuffer::new(config.buffer_size)),
            output_buffers: Arc::new(DoubleBuffer::new(config.buffer_size)),
            config,
            metrics: Arc::new(PipelineMetrics::new()),
            running: Arc::new(AtomicBool::new(false)),
            processing_queue: Arc::new(AsyncMutex::new(VecDeque::new())),
            output_queue: Arc::new(AsyncMutex::new(VecDeque::new())),
            byte_signal: ByteSignal::new(),
        }
    }

    /// Start the pipeline with all three execution contexts
    pub async fn start(&self) -> Result<(), PipelineError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already running
        }

        // Fill the active output buffer
        let zeros = vec![0; self.config.buffer_size];
        self.output_buffers.get_active().await.lock().await.write(&zeros[..self.config.buffer_size]);
        
        let input_handle = self.spawn_input_context();
        info!("input handle started");
        let processing_handle = self.spawn_processing_context();
        info!("processing handle started");
        let output_handle = self.spawn_output_context();
        info!("output handle started");

        // Store handles if needed for graceful shutdown
        tokio::spawn(async move {
            tokio::select! {
                _ = input_handle => {},
                _ = processing_handle => {},
                _ = output_handle => {},
            }
        });

        Ok(())
    }

    /// Stop the pipeline
    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // Signal output thread to wake up and exit
        self.byte_signal.signal_bytes(0, 0);
        // Allow time for contexts to finish
        sleep(Duration::from_millis(100)).await;
    }

    /// Get current metrics
    pub fn metrics(&self) -> PipelineMetrics {
        PipelineMetrics {
            input_bytes: AtomicUsize::new(self.metrics.input_bytes.load(Ordering::Relaxed)),
            output_bytes: AtomicUsize::new(self.metrics.output_bytes.load(Ordering::Relaxed)),
            processing_count: AtomicUsize::new(self.metrics.processing_count.load(Ordering::Relaxed)),
            overflow_count: AtomicUsize::new(self.metrics.overflow_count.load(Ordering::Relaxed)),
            underflow_count: AtomicUsize::new(self.metrics.underflow_count.load(Ordering::Relaxed)),
            average_processing_time_ms: AtomicUsize::new(
                self.metrics.average_processing_time_ms.load(Ordering::Relaxed)
            ),
            buffer_utilization_percent: AtomicUsize::new(
                self.metrics.buffer_utilization_percent.load(Ordering::Relaxed)
            ),
        }
    }

    /// Input context - continuously reads from transport and signals byte counts
    fn spawn_input_context(&self) -> tokio::task::JoinHandle<()> {
        let transport = self.transport.clone();
        let input_buffers = self.input_buffers.clone();
        let processing_queue = self.processing_queue.clone();
        let metrics = self.metrics.clone();
        let running = self.running.clone();
        let config = self.config.clone();
        let byte_signal = self.byte_signal.clone();

        tokio::spawn(async move {
            let mut read_buffer = vec![0u8; config.read_chunk_size];
            let mut caller: usize = 0;

            while running.load(Ordering::Relaxed) {
                // Await new data - no ticker
                let transport_guard = transport.read().await;
                match transport_guard.receive(&mut read_buffer).await {
                    Ok(bytes_read) if bytes_read > 0 => {
                        drop(transport_guard);
                        
                        // Signal the output thread about received bytes
                        byte_signal.signal_bytes(bytes_read, caller);
                        caller = caller.wrapping_add(1);
                        debug!("Input: received {bytes_read} bytes. Requested {bytes_read} bytes to be output");

                        // Write to active input buffer
                        let active_buffer: Arc<AsyncMutex<Buffer>> = input_buffers.get_active().await;
                        let mut buffer = active_buffer.lock().await;
                        
                        let ai = input_buffers.get_active_index();
                        debug!("Input: active Index: {ai}");

                        let written = buffer.write(&read_buffer[..bytes_read]);

                        debug!("Input: {:?}", &read_buffer[..written]);
                        let remaining_bytes = bytes_read - written;
                        metrics.input_bytes.fetch_add(written, Ordering::Relaxed);
                        
                        debug!("Input: Added {written} bytes to input buffer.");
                        if remaining_bytes > 0 {
                            debug!("Input: {remaining_bytes} Bytes need to be added to next active buffer");
                        }


                        // Check if buffer is full and needs swapping
                        if buffer.is_full() {
                            let data = buffer.get_data();
                            buffer.clear();
                            drop(buffer);

                            let len = data.len();
                            debug!("Input: Input buffer is full. Sending {len} bytes to processing");

                            // Queue for processing
                            let mut queue = processing_queue.lock().await;
                            queue.push_back(data);
                            drop(queue);

                            // Attempt buffer swap
                            if let Err(e) = input_buffers.swap().await {
                                metrics.overflow_count.fetch_add(1, Ordering::Relaxed);
                                error!("Input buffer swap failed: {}", e);
                            } else {
                                // Buffer swap successful
                                if remaining_bytes > 0 {
                                    debug!("Adding remaining {remaining_bytes} bytes to new active buffer.");
                                    let new_active = input_buffers.get_active().await;
                                    let mut new_active = new_active.lock().await;

                                    let new_written = new_active.write(&read_buffer[bytes_read-remaining_bytes..bytes_read]);
                                    debug!("New Written: {new_written}");
                                    
                                    // assert!(remaining_bytes >= new_written);
                                    let remaining_bytes = remaining_bytes - new_written;
                                    debug!("Input: Remaining Bytes: {remaining_bytes}");
                                    debug!("Input: New Buffer {:?}", new_active.get_data());
                                }
                            }
                        }
                    }
                    Ok(_) => {
                        // No data available - add small delay to prevent busy loop
                        drop(transport_guard);
                        sleep(Duration::from_millis(1)).await;
                    }
                    Err(e) => {
                        if running.load(Ordering::Relaxed) {
                            warn!("Transport receive error: {}", e);
                        }
                        drop(transport_guard);
                        // Small delay before retrying on error
                        sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        })
    }

    /// Processing context - processes complete buffers
    fn spawn_processing_context(&self) -> tokio::task::JoinHandle<()> {
        let processor = self.processor.clone();
        let processing_queue = self.processing_queue.clone();
        let output_queue = self.output_queue.clone();
        let output_buffers = self.output_buffers.clone();
        let metrics = self.metrics.clone();
        let running = self.running.clone();
        let max_processing_time = self.config.max_processing_time;

        tokio::spawn(async move {
            while running.load(Ordering::Relaxed) {
                // Get data from processing queue
                let data = {
                    let mut queue = processing_queue.lock().await;
                    queue.pop_front()
                };

                if let Some(input_data) = data {
                    let len = input_data.len();
                    debug!("Processing: Dequeued {len} bytes to process.");
                    debug!("Processing: Starting process...");
                    debug!("Processing: {:?}", input_data);
                    let start = Instant::now();

                    // Process with timeout
                    match tokio::time::timeout(
                        max_processing_time,
                        processor.process(input_data)
                    ).await {
                        Ok(Ok(processed_data)) => {
                            // Update metrics
                            let processing_time = start.elapsed();
                            metrics.processing_count.fetch_add(1, Ordering::Relaxed);
                            metrics.average_processing_time_ms.store(
                                processing_time.as_millis() as usize,
                                Ordering::Relaxed
                            );

                            let len = processed_data.len();
                            debug!("Processing: Finished Processing. Output data size: {len} bytes");
                            
                            // Processed data is Vec<u8?
                            let mut queue = output_queue.lock().await;
                            queue.push_back(processed_data);
                            drop(queue);
  
                        }
                        Ok(Err(e)) => {
                            eprintln!("Processing error: {}", e);
                        }
                        Err(_) => {
                            eprintln!("Processing timeout exceeded");
                        }
                    }
                } else {
                    // No data to process, wait a bit
                    sleep(Duration::from_millis(5)).await;
                }
            }
        })
    }

fn spawn_output_context(&self) -> tokio::task::JoinHandle<()> {
    let transport = self.transport.clone();
    let output_buffers = self.output_buffers.clone();
    let output_queue = self.output_queue.clone();
    let metrics = self.metrics.clone();
    let running = self.running.clone();
    let byte_signal = self.byte_signal.clone();

    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            while running.load(Ordering::Relaxed) {
                // Wait for byte signal
                let bytes_to_send = byte_signal.wait_bytes_blocking();
                
                if bytes_to_send == 0 {
                    break; // Shutdown signal
                }

                debug!("Output: {} bytes requested for output", bytes_to_send);

                // CRITICAL: Collect EXACTLY bytes_to_send, no more
                let mut output_data = Vec::with_capacity(bytes_to_send);
                let mut remaining_bytes = bytes_to_send;
                
                // First, try to get data from current active buffer
                {
                    let active_buffer = output_buffers.get_active().await;
                    let mut buffer = active_buffer.lock().await;
                    
                    if !buffer.is_empty() {
                        let available = buffer.fill_level.min(remaining_bytes);
                        let data = buffer.read(available);
                        
                        debug!("Output: Got {} bytes from active buffer", data.len());
                        output_data.extend_from_slice(&data);
                        remaining_bytes -= data.len();
                    }
                }
                
                // If we still need more data, check processing queue
                if remaining_bytes > 0 {
                    // Try to get processed data
                    let processed_data = {
                        let mut queue = output_queue.lock().await;
                        queue.pop_front()
                    };
                    
                    if let Some(data) = processed_data {
                        // Take only what we need
                        let to_take = data.len().min(remaining_bytes);
                        output_data.extend_from_slice(&data[..to_take]);
                        remaining_bytes -= to_take;
                        
                        // If there's leftover data, put it in the buffer for next time
                        if to_take < data.len() {
                            let active_buffer = output_buffers.get_active().await;
                            let mut buffer = active_buffer.lock().await;
                            buffer.write(&data[to_take..]);
                            debug!("Output: Stored {} leftover bytes", data.len() - to_take);
                        }
                    }
                }
                
                // If still missing data, pad with zeros (underflow)
                if remaining_bytes > 0 {
                    debug!("Output: Underflow - padding {} bytes with zeros", remaining_bytes);
                    output_data.resize(bytes_to_send, 0);
                    metrics.underflow_count.fetch_add(1, Ordering::Relaxed);
                }
                
                // Send EXACTLY bytes_to_send
                assert_eq!(output_data.len(), bytes_to_send);
                
                let transport_guard = transport.read().await;
                match transport_guard.send(&output_data).await {
                    Ok(()) => {
                        metrics.output_bytes.fetch_add(output_data.len(), Ordering::Relaxed);
                        debug!("Output: Sent {} bytes", output_data.len());
                        debug!("Output: {:?}", output_data);
                    }
                    Err(e) => {
                        eprintln!("Transport send error: {}", e);
                        break;
                    }
                }
            }
            
            info!("Output context stopped");
        })
    })
}

}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::AsyncMutex;
    use crate::processor::PassThroughProcessor;
    use async_trait::async_trait;
    use env_logger;
    use std::sync::{Once};


    static INIT: Once = Once::new();

    fn init_logger() {
        INIT.call_once(|| {
            env_logger::Builder::from_env(
                env_logger::Env::default().default_filter_or("debug")
            ).init();
        });
    }


    // Mock transport for testing
    #[derive(Clone)]
    struct MockTransport {
        sent_data: Arc<AsyncMutex<Vec<u8>>>,
        receive_data: Arc<AsyncMutex<VecDeque<u8>>>,
    }

    impl MockTransport {
        fn new() -> Self {
            Self {
                sent_data: Arc::new(AsyncMutex::new(Vec::new())),
                receive_data: Arc::new(AsyncMutex::new(VecDeque::new())),
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
    async fn test_buffer_operations() {
        let mut buffer = Buffer::new(10);
        
        // Test writing
        let written = buffer.write(&[1, 2, 3, 4, 5]);
        assert_eq!(written, 5);
        assert_eq!(buffer.fill_level, 5);
        assert!(!buffer.is_full());
        
        // Test reading
        let data = buffer.read(3);
        assert_eq!(data, vec![1, 2, 3]);
        assert_eq!(buffer.fill_level, 2);
        
        // Test filling to capacity
        let written = buffer.write(&[6, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(written, 8); // Should only write 8 bytes to fill buffer
        assert!(buffer.is_full());
    }

    #[tokio::test]
    async fn test_double_buffer_swap() {
        let double_buffer = DoubleBuffer::new(100);
        
        // Initial state
        let active = double_buffer.get_active().await;
        let mut buffer = active.lock().await;
        buffer.write(&[1, 2, 3]);
        drop(buffer);
        
        // Swap buffers
        double_buffer.swap().await.unwrap();
        
        // New active should be empty
        let new_active = double_buffer.get_active().await;
        let buffer = new_active.lock().await;
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn test_byte_signal() {
        let signal = ByteSignal::new();
        
        // Send signal
        signal.signal_bytes(100);
        signal.signal_bytes(200);
        
        // Receive signals
        let bytes1 = signal.wait_bytes().await;
        assert_eq!(bytes1, Some(100));
        
        let bytes2 = signal.wait_bytes().await;
        assert_eq!(bytes2, Some(200));
    }

    #[tokio::test]
    async fn test_pipeline_byte_correspondence() {
        let transport = MockTransport::new();
        let processor = PassThroughProcessor::new(Duration::from_millis(5));

        let config = PipelineConfig {
            buffer_size: 10, // Make buffer smaller so it fills up with our test data
            max_processing_time: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 50,
        };
        
        // Add test data in chunks
        let test_data1 = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]; // 10 bytes to fill buffer
        let test_data2 = vec![11, 12, 13, 14, 15, 16, 17, 18, 19, 20]; // Another 10 bytes
        
        let pipeline = DoubleBufferedIO::new(transport.clone(), processor, config);
        pipeline.start().await.unwrap();
        
        // Add first chunk
        transport.add_input_data(&test_data1).await;
        sleep(Duration::from_millis(200)).await;

        // Add second chunk
        transport.add_input_data(&test_data2).await;
        sleep(Duration::from_millis(300)).await;
        
        // Check metrics - input and output should match
        let metrics = pipeline.metrics();
        let input_bytes = metrics.input_bytes.load(Ordering::Relaxed);
        let output_bytes = metrics.output_bytes.load(Ordering::Relaxed);

        assert_eq!(input_bytes, test_data1.len() + test_data2.len());
        assert_eq!(output_bytes, input_bytes, "Output bytes should equal input bytes");
        
        pipeline.stop().await;
    }

    #[tokio::test]
    async fn test_pipeline_basic_flow() {
        init_logger();


        let transport = MockTransport::new();
        let processor = PassThroughProcessor::new(Duration::from_millis(1));
        
        let config = PipelineConfig {
            buffer_size: 8,
            max_processing_time: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 50,
        };
        

        let pipeline = DoubleBufferedIO::new(transport.clone(), processor, config);
        
        // Start pipeline
        pipeline.start().await.unwrap();
        // Let it run for a bit
        sleep(Duration::from_millis(100)).await;
        let input_data: [u8; 4] = [1,1,1,1];
        transport.add_input_data(&input_data).await;
        sleep(Duration::from_millis(100)).await;
        let input_data: [u8; 4] = [2,2,2,2];
        transport.add_input_data(&input_data).await;
        sleep(Duration::from_millis(100)).await;
        let input_data: [u8; 4] = [3,3,3,3];
        transport.add_input_data(&input_data).await;
        sleep(Duration::from_millis(100)).await;
        let input_data: [u8; 4] = [4,4,4,4];
        transport.add_input_data(&input_data).await;
        sleep(Duration::from_millis(100)).await;
        let input_data: [u8; 4] = [5,5,5,5];
        transport.add_input_data(&input_data).await;
        sleep(Duration::from_millis(100)).await;
        let input_data: [u8; 4] = [6,6,6,6];
        transport.add_input_data(&input_data).await;

        let loopback_data = transport.get_sent_data().await;
        info!("Recevied data: {:?}", loopback_data);

        // Check metrics
        let metrics = pipeline.metrics();
        assert_eq!(metrics.overflow_count.load(Ordering::Relaxed), 0);
        
        // Stop pipeline
        pipeline.stop().await;
    }

    #[tokio::test]
    async fn test_pipeline_streaming_flow() {
        init_logger();

        let transport = MockTransport::new();
        let processor = PassThroughProcessor::new(Duration::from_millis(1));
        
        let config = PipelineConfig {
            buffer_size: 32,
            max_processing_time: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            read_chunk_size: 8,
        };
        

        let pipeline = DoubleBufferedIO::new(transport.clone(), processor, config);
        
        // Start pipeline
        pipeline.start().await.unwrap();
        // Let it run for a bit
        for i in (1..=128).step_by(3) {
            sleep(Duration::from_millis(20)).await;
            let input_data = [i, i + 1, i + 2];
            debug!("Sending: {:?}", input_data);
            transport.add_input_data(&input_data).await;
        }

        let loopback_data: Vec<i8> = transport.get_sent_data().await.into_iter().map(|x| x as i8).collect();
        info!("Recevied data: {:?}", loopback_data);

        for i in 0..loopback_data.len()-1 {
            if loopback_data[i] == 0 && loopback_data[i+1] == 0 {
                continue;
            }
            
            if loopback_data[i] != loopback_data[i+1]-1 {
                error!("Consecutive data mismatch");
                let a = loopback_data[i];
                let b = loopback_data[i+1];
            
                error!("{a} | {b}");
                assert!(false);
            }
        }

        // Check metrics
        let metrics = pipeline.metrics();
        assert_eq!(metrics.overflow_count.load(Ordering::Relaxed), 0);
        
        // Stop pipeline
        pipeline.stop().await;
    }
}