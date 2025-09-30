use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::io::{self, Read, Write};
use std::thread;

/// A thread-safe UART transport with fixed input size
/// 
/// Designed for use with async runtimes like Tokio where tasks may move between threads.
/// Uses RwLock for cheap reads when checking port availability, with writes only when actually using the port.
pub struct UartTransportFixedInput {
    // RwLock allows multiple readers OR one writer
    // Perfect for our use case: one thread reads, one thread writes
    read_port: Arc<RwLock<Option<mio_serial::SerialStream>>>,
    write_port: Arc<RwLock<Option<mio_serial::SerialStream>>>,
    port_name: String,
    baud_rate: u32,
    fixed_receive_size: usize,
    timeout: Arc<RwLock<Duration>>,
}

// Now the struct is automatically Send + Sync
impl Clone for UartTransportFixedInput {
    fn clone(&self) -> Self {
        Self {
            read_port: Arc::clone(&self.read_port),
            write_port: Arc::clone(&self.write_port),
            port_name: self.port_name.clone(),
            baud_rate: self.baud_rate,
            fixed_receive_size: self.fixed_receive_size,
            timeout: Arc::clone(&self.timeout),
        }
    }
}

impl UartTransportFixedInput {
    /// Create a new UART transport for reading with fixed input size
    pub fn new_reader(port_name: &str, baud_rate: u32, fixed_receive_size: usize) -> Result<Self, io::Error> {
        log::debug!("UART: Opening {} at {} baud for fixed {} byte packets (reader mode)",
                   port_name, baud_rate, fixed_receive_size);

        let builder = mio_serial::new(port_name, baud_rate)
            .path(port_name)
            .baud_rate(baud_rate)
            .flow_control(mio_serial::FlowControl::None)
            .data_bits(mio_serial::DataBits::Eight)
            .parity(mio_serial::Parity::None)
            .stop_bits(mio_serial::StopBits::One);

        let mut read_stream = mio_serial::SerialStream::open(&builder)
            .map_err(|e| io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to open mio read stream: {}", e)
            ))?;
        
        // Set to non-exclusive for potential write clone
        match read_stream.set_exclusive(false) {
            Ok(()) => {},
            Err(e) => {
                log::error!("Failed to make mio serial stream non-exclusive: {}", e);
            }
        }

        log::debug!("UART: Successfully created reader handle");

        Ok(Self {
            read_port: Arc::new(RwLock::new(Some(read_stream))),
            write_port: Arc::new(RwLock::new(None)),
            port_name: port_name.to_string(),
            baud_rate,
            fixed_receive_size,
            timeout: Arc::new(RwLock::new(Duration::from_millis(2500))),
        })
    }

    /// Create a writer instance for the same port
    /// This creates a new instance that can be moved to a different thread/task
    pub fn create_writer(&self) -> Result<Self, io::Error> {
        log::debug!("UART: Creating writer for {} at {} baud",
                   self.port_name, self.baud_rate);

        let builder = mio_serial::new(&self.port_name, self.baud_rate)
            .path(&self.port_name)
            .baud_rate(self.baud_rate)
            .flow_control(mio_serial::FlowControl::None)
            .data_bits(mio_serial::DataBits::Eight)
            .parity(mio_serial::Parity::None)
            .stop_bits(mio_serial::StopBits::One);

        let mut write_stream = mio_serial::SerialStream::open(&builder)
            .map_err(|e| io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to open mio write stream: {}", e)
            ))?;

        // Ensure write stream is also non-exclusive
        match write_stream.set_exclusive(false) {
            Ok(()) => {},
            Err(e) => {
                log::error!("Failed to make write stream non-exclusive: {}", e);
            }
        }

        log::debug!("UART: Successfully created write handle");

        Ok(Self {
            read_port: Arc::new(RwLock::new(None)),
            write_port: Arc::new(RwLock::new(Some(write_stream))),
            port_name: self.port_name.clone(),
            baud_rate: self.baud_rate,
            fixed_receive_size: self.fixed_receive_size,
            timeout: Arc::clone(&self.timeout),
        })
    }

    /// Create a bidirectional instance that can both read and write
    pub fn new_bidirectional(port_name: &str, baud_rate: u32, fixed_receive_size: usize) -> Result<Self, io::Error> {
        log::debug!("UART: Opening {} at {} baud for bidirectional communication",
                   port_name, baud_rate);

        let builder = mio_serial::new(port_name, baud_rate)
            .path(port_name)
            .baud_rate(baud_rate)
            .flow_control(mio_serial::FlowControl::None)
            .data_bits(mio_serial::DataBits::Eight)
            .parity(mio_serial::Parity::None)
            .stop_bits(mio_serial::StopBits::One);

        // Open both read and write streams
        let mut read_stream = mio_serial::SerialStream::open(&builder)
            .map_err(|e| io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to open mio read stream: {}", e)
            ))?;
        
        read_stream.set_exclusive(false)
            .map_err(|e| io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to set non-exclusive: {}", e)
            ))?;

        let mut write_stream = mio_serial::SerialStream::open(&builder)
            .map_err(|e| io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to open mio write stream: {}", e)
            ))?;

        write_stream.set_exclusive(false)
            .map_err(|e| io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to set non-exclusive: {}", e)
            ))?;

        Ok(Self {
            read_port: Arc::new(RwLock::new(Some(read_stream))),
            write_port: Arc::new(RwLock::new(Some(write_stream))),
            port_name: port_name.to_string(),
            baud_rate,
            fixed_receive_size,
            timeout: Arc::new(RwLock::new(Duration::from_millis(2500))),
        })
    }

    /// Get current port configuration
    pub fn get_config(&self) -> (String, u32, usize) {
        (
            self.port_name.clone(),
            self.baud_rate,
            self.fixed_receive_size,
        )
    }

    /// Set timeout for operations
    pub fn set_timeout(&self, timeout: Duration) {
        *self.timeout.write().unwrap() = timeout;
    }

    /// Get current timeout
    pub fn timeout(&self) -> Duration {
        *self.timeout.read().unwrap()
    }

    /// Send data through the write port
    pub fn send(&self, data: &[u8]) -> Result<(), io::Error> {
        let mut port_guard = self.write_port.write().unwrap();
        let port = port_guard.as_mut()
            .ok_or_else(|| io::Error::new(
                io::ErrorKind::NotConnected,
                "Write port not available - use create_writer() or new_bidirectional()"
            ))?;
    
        let now = Instant::now();
        
        let mut total_written = 0;
        while total_written < data.len() {
            match port.write(&data[total_written..]) {
                Ok(bytes_written) => {
                    if bytes_written == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "Failed to write any bytes"
                        ));
                    }
                    total_written += bytes_written;
                    
                    // Flush after each successful write
                    port.flush()?;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // For non-blocking I/O, sleep briefly and retry
                    thread::sleep(Duration::from_micros(100));
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        
        log::debug!("UART SEND: Successfully sent {} bytes in {:?}", data.len(), now.elapsed());
        Ok(())
    }

    /// Receive exactly `fixed_receive_size` bytes
    /// Blocks until all the data is received
    pub fn receive_full(&self, buffer: &mut [u8]) -> Result<usize, io::Error> {
        if self.fixed_receive_size > buffer.len() {
            panic!("Fixed receive size ({}) exceeds buffer length ({})", 
                   self.fixed_receive_size, buffer.len());
        }

        let mut port_guard = self.read_port.write().unwrap();
        let port = port_guard.as_mut()
            .ok_or_else(|| io::Error::new(
                io::ErrorKind::NotConnected,
                "Read port not available - this instance may be write-only"
            ))?;

        let mut total_received = 0;

        while total_received < self.fixed_receive_size {
            let remaining = self.fixed_receive_size - total_received;
            //log::debug!("UART RECEIVE: Waiting for {} more bytes", remaining);
            let buffer_slice = &mut buffer[total_received..total_received + remaining];
            
            match port.read(buffer_slice) {
                Ok(bytes_read) => {
                    if bytes_read == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Connection closed before receiving all data"
                        ));
                    }
                    total_received += bytes_read;
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::WouldBlock {
                        thread::sleep(Duration::from_micros(100));
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        
        log::debug!("UART RECEIVE: Successfully received {} bytes", total_received);
        Ok(self.fixed_receive_size)
    }

    /// Receive data with timeout (returns number of bytes read)
    pub fn receive_with_timeout(&self, buffer: &mut [u8]) -> Result<usize, io::Error> {
        let timeout = self.timeout();
        let start = Instant::now();
        
        let mut port_guard = self.read_port.write().unwrap();
        let port = port_guard.as_mut()
            .ok_or_else(|| io::Error::new(
                io::ErrorKind::NotConnected,
                "Read port not available"
            ))?;

        let mut total_received = 0;
        let max_to_read = buffer.len().min(self.fixed_receive_size);

        while total_received < max_to_read && start.elapsed() < timeout {
            let buffer_slice = &mut buffer[total_received..max_to_read];
            
            match port.read(buffer_slice) {
                Ok(bytes_read) => {
                    if bytes_read == 0 {
                        return Ok(total_received);
                    }
                    total_received += bytes_read;
                    if total_received >= self.fixed_receive_size {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if start.elapsed() >= timeout {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("Timeout after receiving {} bytes", total_received)
                        ));
                    }
                    thread::sleep(Duration::from_micros(100));
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(total_received)
    }

    /// Check if this instance has read capability
    pub fn can_read(&self) -> bool {
        self.read_port.read().unwrap().is_some()
    }

    /// Check if this instance has write capability
    pub fn can_write(&self) -> bool {
        self.write_port.read().unwrap().is_some()
    }
}