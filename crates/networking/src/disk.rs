use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fusion_types::StreamableData;

/// A disk writer for recording data to files as JSON lines.
pub struct DiskWriter {
    m_path: String,
    m_count: Arc<Mutex<usize>>,
    m_writer: Mutex<Option<BufWriter<File>>>,
}

impl DiskWriter {
    pub fn new(path: impl Into<String>) -> Self {
        let path = path.into();
        let writer = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => Some(BufWriter::new(file)),
            Err(e) => {
                log::error!("DiskWriter: failed to open '{}': {}", path, e);
                None
            }
        };

        Self {
            m_path: path,
            m_count: Arc::new(Mutex::new(0)),
            m_writer: Mutex::new(writer),
        }
    }

    pub fn path(&self) -> &str {
        &self.m_path
    }

    /// Write a streamable data entry as a JSON line.
    pub fn write(&self, data: &StreamableData) -> Result<()> {
        let json = serde_json::to_string(data)?;
        let mut writer_guard = self.m_writer.lock().unwrap();
        if let Some(ref mut w) = *writer_guard {
            writeln!(w, "{}", json)?;
            w.flush()?;
            let mut count = self.m_count.lock().unwrap();
            *count += 1;
            Ok(())
        } else {
            Err(anyhow::anyhow!("DiskWriter: file not opened for '{}'", self.m_path))
        }
    }

    pub fn count(&self) -> usize {
        *self.m_count.lock().unwrap()
    }
}

/// A disk reader for reading recorded data files (JSON lines).
pub struct DiskReader {
    m_path: String,
}

impl DiskReader {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            m_path: path.into(),
        }
    }

    pub fn path(&self) -> &str {
        &self.m_path
    }

    /// Read all data entries, calling the callback for each one.
    pub fn read<F>(&self, mut callback: F) -> Result<()>
    where
        F: FnMut(StreamableData),
    {
        let file = File::open(&self.m_path)?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(data) = serde_json::from_str::<StreamableData>(&line) {
                callback(data);
            }
        }
        Ok(())
    }
}
