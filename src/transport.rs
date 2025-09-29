// transport.rs

use async_trait::async_trait;
use std::time::Duration;
use std::net::SocketAddr;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use std::sync::Arc;
use thiserror::Error;
use std::time::{Instant};
use log::{debug, info};

/// Error types for the transport layer
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serial port error: {0}")]
    SerialPort(#[from] tokio_serial::Error),
    #[error("Timeout occurred")]
    Timeout,
    #[error("Transport not connected")]
    NotConnected,
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    #[error("Buffer too small")]
    BufferTooSmall,
}

/// Trait defining the transport layer interface
/// 
/// This trait abstracts the underlying communication mechanism, allowing for both
/// real UDP transport and mock implementations for testing.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send data to the target device
    /// 
    /// # Arguments
    /// 
    /// * `data` - The bytes to send
    /// 
    /// # Returns
    /// 
    /// * `Result<(), Error>` - Ok(()) on success, error on failure
    async fn send(&self, data: &[u8]) -> Result<(), Error>;
    
    /// Receive data from the device
    ///
    /// # Arguments
    ///
    /// * `buffer` - Buffer to store received data
    ///
    /// # Returns
    ///
    /// * `Result<usize, Error>` - Number of bytes received or error
    async fn receive(&self, buffer: &mut [u8]) -> Result<usize, Error>;

    /// Receive data from the device with sender address information
    ///
    /// # Arguments
    ///
    /// * `buffer` - Buffer to store received data
    ///
    /// # Returns
    ///
    /// * `Result<(usize, SocketAddr), Error>` - Number of bytes received and sender address or error
    async fn receive_from(&self, buffer: &mut [u8]) -> Result<(usize, SocketAddr), Error>;

    /// Send data to a specific address
    ///
    /// # Arguments
    ///
    /// * `data` - The bytes to send
    /// * `addr` - Target address to send to
    ///
    /// # Returns
    ///
    /// * `Result<(), Error>` - Ok(()) on success, error on failure
    async fn send_to(&self, data: &[u8], addr: SocketAddr) -> Result<(), Error>;

    /// Set the default timeout for operations
    ///
    /// # Arguments
    ///
    /// * `timeout` - The timeout duration to set
    fn set_timeout(&mut self, timeout: Duration);
}

/// UART Transport implementation using tokio_serial
pub struct UartTransport {
    port: Arc<Mutex<SerialStream>>,
    timeout: Arc<Mutex<Duration>>,
    port_name: String,
    baud_rate: u32,
}

impl UartTransport {
    /// Create a new UART transport
    /// 
    /// # Arguments
    /// 
    /// * `port_name` - Serial port name (e.g., "/dev/ttyUSB0" on Linux, "COM3" on Windows)
    /// * `baud_rate` - Baud rate for the serial connection
    /// 
    /// # Returns
    /// 
    /// * `Result<Self, Error>` - New UartTransport instance or error
    pub async fn new(port_name: &str, baud_rate: u32) -> Result<Self, Error> {
        let port = tokio_serial::new(port_name, baud_rate)
            .timeout(Duration::from_secs(1))
            .open_native_async()?;

        Ok(Self {
            port: Arc::new(Mutex::new(port)),
            timeout: Arc::new(Mutex::new(Duration::from_secs(5))),
            port_name: port_name.to_string(),
            baud_rate,
        })
    }

    /// Reconfigure the serial port with new settings
    /// 
    /// # Arguments
    /// 
    /// * `baud_rate` - New baud rate
    /// 
    /// # Returns
    /// 
    /// * `Result<(), Error>` - Ok on success
    pub async fn reconfigure(&mut self, baud_rate: u32) -> Result<(), Error> {
        self.baud_rate = baud_rate;
        
        // Close existing port and reopen with new settings
        let new_port = tokio_serial::new(&self.port_name, baud_rate)
            .timeout(*self.timeout.lock().await)
            .open_native_async()?;
            
        *self.port.lock().await = new_port;
        Ok(())
    }

    /// Get current port configuration
    pub async fn get_config(&self) -> (String, u32, Duration) {
        (
            self.port_name.clone(),
            self.baud_rate,
            *self.timeout.lock().await
        )
    }
}

#[async_trait]
impl Transport for UartTransport {
        async fn send(&self, data: &[u8]) -> Result<(), Error> {
        let mut port = self.port.lock().await;
        let timeout = *self.timeout.lock().await;
        
        let now = Instant::now();
        
        // Use eprintln! for immediate output to stderr (unbuffered)
        eprintln!("UART SEND: {} bytes at {:?}", data.len(), now);
        eprintln!("  First 20 bytes: {:?}", &data[..data.len().min(20)]);
        
        // Also use the log system
        debug!("UART SEND: {} bytes, data: {:?}", data.len(), &data[..data.len().min(50)]);
        
        match tokio::time::timeout(timeout, port.write_all(data)).await {
            Ok(Ok(())) => {
                // Ensure data is flushed
                port.flush().await?;
                eprintln!("UART SEND COMPLETE: {} bytes successfully sent", data.len());
                Ok(())
            }
            Ok(Err(e)) => {
                eprintln!("UART SEND ERROR: {}", e);
                Err(Error::Io(e))
            }
            Err(_) => {
                eprintln!("UART SEND TIMEOUT after {:?}", timeout);
                Err(Error::Timeout)
            }
        }
    }

    async fn receive(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        let mut port = self.port.lock().await;
        let timeout = *self.timeout.lock().await;
        
        match tokio::time::timeout(timeout, port.read(buffer)).await {
            Ok(Ok(bytes_read)) => Ok(bytes_read),
            Ok(Err(e)) => {
                // Handle the case where no data is available
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    Ok(0)
                } else {
                    Err(Error::Io(e))
                }
            }
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn receive_from(&self, buffer: &mut [u8]) -> Result<(usize, SocketAddr), Error> {
        // UART doesn't have addressing like UDP, so we'll return a dummy address
        let bytes = self.receive(buffer).await?;
        let dummy_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        Ok((bytes, dummy_addr))
    }

    async fn send_to(&self, data: &[u8], _addr: SocketAddr) -> Result<(), Error> {
        // UART doesn't use addressing, so we ignore the address parameter
        self.send(data).await
    }

    fn set_timeout(&mut self, timeout: Duration) {
        // We need to handle the async mutex properly
        let timeout_mutex = self.timeout.clone();
        tokio::spawn(async move {
            *timeout_mutex.lock().await = timeout;
        });
    }
}

/// Builder pattern for UART transport configuration
pub struct UartTransportBuilder {
    port_name: String,
    baud_rate: u32,
    timeout: Duration,
    data_bits: tokio_serial::DataBits,
    parity: tokio_serial::Parity,
    stop_bits: tokio_serial::StopBits,
    flow_control: tokio_serial::FlowControl,
}

impl UartTransportBuilder {
    /// Create a new builder with default settings
    pub fn new(port_name: &str) -> Self {
        Self {
            port_name: port_name.to_string(),
            baud_rate: 115200,
            timeout: Duration::from_secs(5),
            data_bits: tokio_serial::DataBits::Eight,
            parity: tokio_serial::Parity::None,
            stop_bits: tokio_serial::StopBits::One,
            flow_control: tokio_serial::FlowControl::None,
        }
    }

    /// Set baud rate
    pub fn baud_rate(mut self, baud_rate: u32) -> Self {
        self.baud_rate = baud_rate;
        self
    }

    /// Set timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set data bits
    pub fn data_bits(mut self, data_bits: tokio_serial::DataBits) -> Self {
        self.data_bits = data_bits;
        self
    }

    /// Set parity
    pub fn parity(mut self, parity: tokio_serial::Parity) -> Self {
        self.parity = parity;
        self
    }

    /// Set stop bits
    pub fn stop_bits(mut self, stop_bits: tokio_serial::StopBits) -> Self {
        self.stop_bits = stop_bits;
        self
    }

    /// Set flow control
    pub fn flow_control(mut self, flow_control: tokio_serial::FlowControl) -> Self {
        self.flow_control = flow_control;
        self
    }

    /// Build the UART transport
    pub async fn build(self) -> Result<UartTransport, Error> {
        let port = tokio_serial::new(&self.port_name, self.baud_rate)
            .data_bits(self.data_bits)
            .parity(self.parity)
            .stop_bits(self.stop_bits)
            .flow_control(self.flow_control)
            .timeout(self.timeout)
            .open_native_async()?;

        Ok(UartTransport {
            port: Arc::new(Mutex::new(port)),
            timeout: Arc::new(Mutex::new(self.timeout)),
            port_name: self.port_name,
            baud_rate: self.baud_rate,
        })
    }
}

// UDP Transport implementation for comparison/testing
pub struct UdpTransport {
    socket: Arc<tokio::net::UdpSocket>,
    remote_addr: Option<SocketAddr>,
    timeout: Duration,
}

impl UdpTransport {
    /// Create a new UDP transport
    pub async fn new(local_addr: &str, remote_addr: Option<&str>) -> Result<Self, Error> {
        let socket = tokio::net::UdpSocket::bind(local_addr).await?;
        let remote = remote_addr.map(|addr| addr.parse()).transpose()
            .map_err(|e| Error::InvalidOperation(format!("Invalid address: {}", e)))?;
        
        Ok(Self {
            socket: Arc::new(socket),
            remote_addr: remote,
            timeout: Duration::from_secs(5),
        })
    }

    /// Connect to a remote address
    pub async fn connect(&mut self, addr: &str) -> Result<(), Error> {
        let addr: SocketAddr = addr.parse()
            .map_err(|e| Error::InvalidOperation(format!("Invalid address: {}", e)))?;
        self.socket.connect(addr).await?;
        self.remote_addr = Some(addr);
        Ok(())
    }
}

#[async_trait]
impl Transport for UdpTransport {
    async fn send(&self, data: &[u8]) -> Result<(), Error> {
        if let Some(addr) = self.remote_addr {
            self.socket.send_to(data, addr).await?;
        } else {
            return Err(Error::NotConnected);
        }
        Ok(())
    }

    async fn receive(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        match tokio::time::timeout(self.timeout, self.socket.recv(buffer)).await {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(e)) => Err(Error::Io(e)),
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn receive_from(&self, buffer: &mut [u8]) -> Result<(usize, SocketAddr), Error> {
        match tokio::time::timeout(self.timeout, self.socket.recv_from(buffer)).await {
            Ok(Ok((bytes, addr))) => Ok((bytes, addr)),
            Ok(Err(e)) => Err(Error::Io(e)),
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn send_to(&self, data: &[u8], addr: SocketAddr) -> Result<(), Error> {
        self.socket.send_to(data, addr).await?;
        Ok(())
    }

    fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }
}

pub struct UartTransportFixedInput {
    port: Arc<Mutex<tokio_serial::SerialStream>>,
    timeout: Arc<Mutex<Duration>>,
    port_name: String,
    baud_rate: u32,
    fixed_receive_size: usize,
}

impl UartTransportFixedInput {
    /// Create a new UART transport with fixed input size
    /// 
    /// # Arguments
    /// 
    /// * `port_name` - Serial port name (e.g., "/dev/ttyUSB0" on Linux, "COM3" on Windows)
    /// * `baud_rate` - Baud rate for the serial connection
    /// * `fixed_receive_size` - Fixed number of bytes to receive in each receive() call
    /// 
    /// # Returns
    /// 
    /// * `Result<Self, Error>` - New UartTransportFixedInput instance or error
    pub async fn new(port_name: &str, baud_rate: u32, fixed_receive_size: usize) -> Result<Self, Error> {
        let port = tokio_serial::new(port_name, baud_rate)
            .timeout(Duration::from_secs(1))
            .open_native_async()?;

        Ok(Self {
            port: Arc::new(Mutex::new(port)),
            timeout: Arc::new(Mutex::new(Duration::from_secs(5))),
            port_name: port_name.to_string(),
            baud_rate,
            fixed_receive_size,
        })
    }

    /// Reconfigure the serial port with new settings
    /// 
    /// # Arguments
    /// 
    /// * `baud_rate` - New baud rate
    /// 
    /// # Returns
    /// 
    /// * `Result<(), Error>` - Ok on success
    pub async fn reconfigure(&mut self, baud_rate: u32) -> Result<(), Error> {
        self.baud_rate = baud_rate;
        
        // Close existing port and reopen with new settings
        let new_port = tokio_serial::new(&self.port_name, baud_rate)
            .timeout(*self.timeout.lock().await)
            .open_native_async()?;
            
        *self.port.lock().await = new_port;
        Ok(())
    }

    /// Get current port configuration
    pub fn get_config(&self) -> (String, u32, usize) {
        (
            self.port_name.clone(),
            self.baud_rate,
            self.fixed_receive_size,
        )
    }

    /// Receive exactly `fixed_receive_size` bytes, blocking until all are received
    /// 
    /// # Arguments
    /// 
    /// * `buffer` - Buffer to store received data (must be at least `fixed_receive_size` bytes)
    /// 
    /// # Returns
    /// 
    /// * `Result<(), Error>` - Ok when exactly `fixed_receive_size` bytes have been received
    /// 
    /// # Panics
    /// 
    /// Panics if `fixed_receive_size` is greater than `buffer.len()`
    pub async fn receive_full(&self, buffer: &mut [u8]) -> Result<(), Error> {
        if self.fixed_receive_size > buffer.len() {
            panic!("Fixed receive size ({}) exceeds buffer length ({})", 
                   self.fixed_receive_size, buffer.len());
        }

        let mut port = self.port.lock().await;
        let timeout = *self.timeout.lock().await;
        let mut total_received = 0;

        while total_received < self.fixed_receive_size {
            let remaining = self.fixed_receive_size - total_received;
            let buffer_slice = &mut buffer[total_received..total_received + remaining];
            
            match tokio::time::timeout(timeout, port.read(buffer_slice)).await {
                Ok(Ok(bytes_read)) => {
                    if bytes_read == 0 {
                        // Handle EOF or connection closed
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "Connection closed before receiving all data"
                        )));
                    }
                    total_received += bytes_read;
                }
                Ok(Err(e)) => {
                    // Handle non-blocking case differently - retry instead of returning error
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        // Small delay to prevent busy waiting
                        tokio::time::sleep(Duration::from_millis(1)).await;
                        continue;
                    } else {
                        return Err(Error::Io(e));
                    }
                }
                Err(_) => return Err(Error::Timeout),
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Transport for UartTransportFixedInput {
    async fn send(&self, data: &[u8]) -> Result<(), Error> {
        let mut port = self.port.lock().await;
        let timeout = *self.timeout.lock().await;
        let now = Instant::now();

        match tokio::time::timeout(timeout, port.write_all(data)).await {
            Ok(Ok(())) => {
                // Ensure data is flushed
                port.flush().await?;
                debug!("UART SEND: {} bytes at {:?}", data.len(), now);
                Ok(())
            }
            Ok(Err(e)) => Err(Error::Io(e)),
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn receive(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        // Use the fixed-size receive implementation
        self.receive_full(buffer).await?;
        Ok(self.fixed_receive_size)
    }

    async fn receive_from(&self, buffer: &mut [u8]) -> Result<(usize, SocketAddr), Error> {
        Err(Error::InvalidOperation("receive_from not implemented".to_string()))
    }

    async fn send_to(&self, data: &[u8], addr: SocketAddr) -> Result<(), Error> {
        Err(Error::InvalidOperation("send_to not implemented".to_string()))
    }

    fn set_timeout(&mut self, timeout: Duration) {
        let timeout_mutex = self.timeout.clone();
        tokio::spawn(async move {
            *timeout_mutex.lock().await = timeout;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_uart_builder() {
        let builder = UartTransportBuilder::new("/dev/ttyUSB0")
            .baud_rate(9600)
            .timeout(Duration::from_secs(10))
            .data_bits(tokio_serial::DataBits::Seven)
            .parity(tokio_serial::Parity::Even)
            .stop_bits(tokio_serial::StopBits::Two)
            .flow_control(tokio_serial::FlowControl::Software);

        // Note: This will fail unless you have a serial port available
        // In real tests, you'd use a mock or virtual serial port
        match builder.build().await {
            Ok(_transport) => {
                // Transport created successfully
            }
            Err(_e) => {
                // Expected in test environment without actual serial port
            }
        }
    }

    #[tokio::test]
    async fn test_udp_transport() {
        let transport = UdpTransport::new("127.0.0.1:0", Some("127.0.0.1:8080"))
            .await
            .expect("Failed to create UDP transport");

        // Test basic operations
        let data = b"Hello, World!";
        
        // This will fail without a listener on the other end, but demonstrates usage
        let _ = transport.send(data).await;
    }
}