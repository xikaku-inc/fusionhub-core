use anyhow::Result;
use fusion_types::StreamableData;

use crate::publisher::Publisher;

/// A writer that encodes data and publishes it over a network endpoint.
/// Thin wrapper around [`Publisher`].
pub struct NetworkWriter {
    m_publisher: Publisher,
}

impl NetworkWriter {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            m_publisher: Publisher::new(endpoint),
        }
    }

    pub fn endpoint(&self) -> &str {
        self.m_publisher.endpoint()
    }

    pub fn store(&self, data: &StreamableData) -> Result<()> {
        self.m_publisher.publish(data)
    }
}
