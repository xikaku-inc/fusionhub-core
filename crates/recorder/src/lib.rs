#![allow(dead_code)]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fusion_types::StreamableData;

// ---------------------------------------------------------------------------
// JsonEncoder (matches C++ Fusion::JsonEncoder::encode)
// ---------------------------------------------------------------------------

/// Encode a StreamableData value as a JSON string (one line, no trailing newline).
pub fn json_encode(data: &StreamableData) -> String {
    serde_json::to_string(data).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// WriteToStream
// ---------------------------------------------------------------------------

/// Writes JSON-encoded data lines to a file.  Thread-safe.
pub struct WriteToStream {
    m_writer: Arc<Mutex<BufWriter<File>>>,
    m_count: Arc<AtomicUsize>,
}

impl WriteToStream {
    /// Open an output file for writing.
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            m_writer: Arc::new(Mutex::new(BufWriter::new(file))),
            m_count: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Write a single data entry as a JSON line.
    pub fn write(&self, data: &StreamableData) -> std::io::Result<()> {
        let line = json_encode(data);
        let mut writer = self.m_writer.lock().unwrap();
        writeln!(writer, "{}", line)?;
        self.m_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Flush the underlying buffer to disk.
    pub fn flush(&self) -> std::io::Result<()> {
        let mut writer = self.m_writer.lock().unwrap();
        writer.flush()
    }

    /// Return the total number of entries written so far.
    pub fn count(&self) -> usize {
        self.m_count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Recorder
// ---------------------------------------------------------------------------

/// Records fusion data from one or more network endpoints to an NDJSON file.
///
/// This is the Rust port of the C++ `mainRecorder.cpp`.
pub struct Recorder {
    m_output_path: String,
    m_endpoints: Vec<String>,
    m_writer: Option<WriteToStream>,
    m_subscriber: Option<networking::Subscriber>,
    m_interrupted: Arc<AtomicBool>,
}

impl Recorder {
    /// Create a new recorder.
    ///
    /// * `output_path` -- path to the output NDJSON file.
    /// * `endpoints` -- list of ZMQ endpoint strings to subscribe to.
    pub fn new(output_path: &str, endpoints: Vec<String>) -> Self {
        Self {
            m_output_path: output_path.to_owned(),
            m_endpoints: endpoints,
            m_writer: None,
            m_subscriber: None,
            m_interrupted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get a handle to the interrupt flag.  Set to `true` to stop recording.
    pub fn interrupt_flag(&self) -> Arc<AtomicBool> {
        self.m_interrupted.clone()
    }

    /// Start recording.  This function blocks until the interrupt flag is set.
    pub fn run(&mut self) -> anyhow::Result<()> {
        let path = Path::new(&self.m_output_path);
        let writer = WriteToStream::new(path)
            .map_err(|e| anyhow::anyhow!("Could not open output file: {}", e))?;

        log::info!("Recording to {}", self.m_output_path);
        log::info!("Endpoints: {:?}", self.m_endpoints);

        let subscriber = networking::Subscriber::new(self.m_endpoints.clone());

        // In the full implementation, start_listening would receive data from
        // ZMQ sockets and call writer.write() for each message.  Here we set up
        // the scaffolding.
        let writer_ref = Arc::new(writer);
        let wr = writer_ref.clone();
        subscriber.start_listening(move |data: StreamableData| {
            if let Err(e) = wr.write(&data) {
                log::error!("Write error: {}", e);
            }
        })?;

        self.m_subscriber = Some(subscriber);

        log::info!("Running, press Ctrl+C to quit");

        while !self.m_interrupted.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(1));
            eprint!("\rRunning, handled {} packets", writer_ref.count());
        }
        eprintln!();

        writer_ref.flush()?;
        log::info!("Processed {} packets total", writer_ref.count());

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::Timestamp;

    #[test]
    fn json_encode_roundtrip() {
        let data = StreamableData::Timestamp(Timestamp::current());
        let encoded = json_encode(&data);
        assert!(!encoded.is_empty());
        let decoded: StreamableData = serde_json::from_str(&encoded).unwrap();
        match decoded {
            StreamableData::Timestamp(_) => {}
            _ => panic!("Expected Timestamp variant"),
        }
    }

    #[test]
    fn write_to_stream_basic() {
        let dir = std::env::temp_dir();
        let path = dir.join("recorder_test.ndjson");
        let writer = WriteToStream::new(&path).unwrap();
        let data = StreamableData::Timestamp(Timestamp::current());
        writer.write(&data).unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.count(), 1);
        std::fs::remove_file(&path).ok();
    }
}
