// processor.rs

use async_trait::async_trait;
use std::time::Duration;
use tokio::time::sleep;
use crate::double_buffered_io::PipelineError;

/// Generic data processor trait
#[async_trait]
pub trait DataProcessor: Send + Sync {
    /// Process a buffer of data
    ///
    /// # Arguments
    /// * `input` - Input data buffer
    ///
    /// # Returns
    /// * Processed data
    async fn process(&self, input: Vec<u8>) -> Result<Vec<u8>, PipelineError>;
}

/// Pass-through processor that just delays and returns input
pub struct PassThroughProcessor {
    delay: Duration,
}

impl PassThroughProcessor {
    pub fn new(delay: Duration) -> Self {
        Self { delay }
    }
}

#[async_trait]
impl DataProcessor for PassThroughProcessor {
    async fn process(&self, input: Vec<u8>) -> Result<Vec<u8>, PipelineError> {
        if self.delay > Duration::ZERO {
            sleep(self.delay).await;
        }
        Ok(input)
    }
}

/// AddOneProcessor that adds one to all elements in the input data
pub struct AddOneProcessor;

impl AddOneProcessor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl DataProcessor for AddOneProcessor {
    async fn process(&self, input: Vec<u8>) -> Result<Vec<u8>, PipelineError> {
        let output = input.into_iter().map(|b| b.wrapping_add(1)).collect();
        sleep(Duration::from_millis(0)).await;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pass_through_processor() {
        let processor = PassThroughProcessor::new(Duration::from_millis(1));
        let input = vec![1, 2, 3, 4, 5];
        let output = processor.process(input.clone()).await.unwrap();
        assert_eq!(input, output);
    }

    #[tokio::test]
    async fn test_add_one_processor() {
        let processor = AddOneProcessor::new();
        let input = vec![1, 2, 3, 254, 255];
        let output = processor.process(input).await.unwrap();
        assert_eq!(output, vec![2, 3, 4, 255, 0]); // 255 + 1 wraps to 0
    }
}