//! Double-buffered I/O pipeline library
//! 
//! This library provides a high-performance, event-driven I/O pipeline
//! with double-buffering for continuous data flow.

pub mod transport;
pub mod double_buffered_io;
pub mod processor;

// Re-export main types for convenience
pub use transport::{Transport, UartTransport, UdpTransport, Error};
pub use processor::{DataProcessor, PassThroughProcessor, AddOneProcessor};
pub use double_buffered_io::{
    DoubleBufferedIO,
    PipelineConfig,
    PipelineMetrics,
    PipelineError,
};