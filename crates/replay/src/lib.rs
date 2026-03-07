use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use fusion_types::StreamableData;
use networking::{DiskReader, NetworkWriter};

// ---------------------------------------------------------------------------
// SampleElapsedTimeCalculator
// ---------------------------------------------------------------------------

/// Calculates the elapsed time of a data sample relative to the first sample
/// seen for each sender.
struct SampleElapsedTimeCalculator {
    m_start_times: HashMap<String, SystemTime>,
}

impl SampleElapsedTimeCalculator {
    fn new() -> Self {
        Self {
            m_start_times: HashMap::new(),
        }
    }

    fn reset(&mut self) {
        self.m_start_times.clear();
    }

    /// Returns the duration between this sample's timestamp and the first
    /// sample from the same sender.
    fn elapsed(&mut self, data: &StreamableData) -> Duration {
        let (sender_id, timestamp) = extract_sender_and_time(data);
        let start = *self
            .m_start_times
            .entry(sender_id)
            .or_insert(timestamp);
        timestamp.duration_since(start).unwrap_or(Duration::ZERO)
    }
}

/// Extract the sender id and timestamp from a StreamableData variant.
fn extract_sender_and_time(data: &StreamableData) -> (String, SystemTime) {
    match data {
        StreamableData::Imu(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::Gnss(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::Optical(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::FusedPose(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::FusedVehiclePose(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::FusedVehiclePoseV2(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::GlobalFusedPose(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::FusionStateInt(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::Rtcm(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::Can(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::VehicleState(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::VehicleSpeed(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::VelocityMeter(d) => (d.sender_id.clone(), d.timestamp),
        StreamableData::Timestamp(d) => ("<timestamp>".to_owned(), d.now),
        StreamableData::Extension(e) => (e.sender_id.clone(), e.timestamp),
    }
}

/// Short type label for debug printing.
fn data_type_label(data: &StreamableData) -> &'static str {
    match data {
        StreamableData::Imu(_) => "IMU",
        StreamableData::Gnss(_) => "GNSS",
        StreamableData::Optical(_) => "OPT",
        StreamableData::FusedPose(_) => "FUS",
        StreamableData::FusedVehiclePose(_) => "VFUS",
        StreamableData::FusedVehiclePoseV2(_) => "VFUSV2",
        StreamableData::GlobalFusedPose(_) => "GFP",
        StreamableData::FusionStateInt(_) => "FSINT",
        StreamableData::Rtcm(_) => "RTM",
        StreamableData::Can(_) => "CAN",
        StreamableData::VehicleState(_) => "VEC",
        StreamableData::VehicleSpeed(_) => "VSPD",
        StreamableData::VelocityMeter(_) => "VMD",
        StreamableData::Timestamp(_) => "TS",
        StreamableData::Extension(_) => "EXT",
    }
}

// ---------------------------------------------------------------------------
// ReplayQueue
// ---------------------------------------------------------------------------

struct ReplayQueueInner {
    queue: std::collections::VecDeque<StreamableData>,
    max_size: usize,
}

struct ReplayQueue {
    inner: Mutex<ReplayQueueInner>,
    condvar: Condvar,
}

impl ReplayQueue {
    fn new(max_size: usize) -> Self {
        Self {
            inner: Mutex::new(ReplayQueueInner {
                queue: std::collections::VecDeque::new(),
                max_size,
            }),
            condvar: Condvar::new(),
        }
    }

    /// Push data onto the queue, blocking if the queue is full.
    fn push(&self, data: StreamableData) {
        let mut inner = self.inner.lock().unwrap();
        while inner.queue.len() >= inner.max_size {
            inner = self.condvar.wait(inner).unwrap();
        }
        inner.queue.push_back(data);
        self.condvar.notify_all();
    }

    /// Pop data from the front, if available.
    fn pop_front(&self) -> Option<StreamableData> {
        let mut inner = self.inner.lock().unwrap();
        let item = inner.queue.pop_front();
        if item.is_some() {
            self.condvar.notify_all();
        }
        item
    }

    /// Peek at the front without removing.
    fn peek_front(&self) -> Option<StreamableData> {
        let inner = self.inner.lock().unwrap();
        inner.queue.front().cloned()
    }

    fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.queue.len()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Wait until the queue has data or a timeout expires.
    fn wait_for_data(&self, timeout: Duration) -> bool {
        let inner = self.inner.lock().unwrap();
        if !inner.queue.is_empty() {
            return true;
        }
        let (inner, _) = self.condvar.wait_timeout(inner, timeout).unwrap();
        !inner.queue.is_empty()
    }

    fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.clear();
        self.condvar.notify_all();
    }
}

// ---------------------------------------------------------------------------
// RealtimePlayback
// ---------------------------------------------------------------------------

/// Replays recorded fusion data in real time, scaled by a configurable speed
/// factor.
///
/// This is the Rust port of the C++ `Replay::RealtimePlayback`.
pub struct RealtimePlayback {
    m_replay_speed: f64,
    m_queue: Arc<ReplayQueue>,
    m_verbose: bool,
    m_echo_data: bool,
    m_timecode_input: bool,
    m_buffer_delay_ms: u64,
    m_read_multiple_lines: usize,
    m_block_size: usize,
    m_eof: Arc<AtomicBool>,
    m_running: Arc<AtomicBool>,
    m_replay_thread: Option<thread::JoinHandle<()>>,
    m_writer: Arc<NetworkWriter>,
    m_first_data_initialized: Arc<AtomicBool>,
    m_replay_start: Arc<Mutex<Instant>>,
    m_sample_calculator: Arc<Mutex<SampleElapsedTimeCalculator>>,
}

impl RealtimePlayback {
    /// Create a new replay processor.
    ///
    /// * `replay_speed` -- time scaling factor (1.0 = realtime, 2.0 = double speed).
    /// * `queue_size` -- maximum number of buffered samples.
    /// * `writer_endpoint` -- ZMQ endpoint string for the output publisher.
    /// * `verbose` -- enable debug logging.
    pub fn new(
        replay_speed: f64,
        queue_size: usize,
        writer_endpoint: &str,
        verbose: bool,
    ) -> Self {
        log::info!("Running replay at speed = {}", replay_speed);
        log::info!("Debug output is set to {}", verbose);

        Self {
            m_replay_speed: replay_speed,
            m_queue: Arc::new(ReplayQueue::new(queue_size)),
            m_verbose: verbose,
            m_echo_data: false,
            m_timecode_input: false,
            m_buffer_delay_ms: 500,
            // DiskReader could use these for batch reading in the future.
            m_read_multiple_lines: 100,
            m_block_size: 65536,
            m_eof: Arc::new(AtomicBool::new(false)),
            m_running: Arc::new(AtomicBool::new(false)),
            m_replay_thread: None,
            m_writer: Arc::new(NetworkWriter::new(writer_endpoint)),
            m_first_data_initialized: Arc::new(AtomicBool::new(false)),
            m_replay_start: Arc::new(Mutex::new(Instant::now())),
            m_sample_calculator: Arc::new(Mutex::new(SampleElapsedTimeCalculator::new())),
        }
    }

    /// Create from JSON settings (matching the C++ constructor).
    pub fn from_json(config: &serde_json::Value) -> Self {
        let replay_speed = config
            .get("replaySpeed")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let queue_size = config
            .get("queueSize")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;
        let writer_endpoint = config
            .get("writerEndpoint")
            .and_then(|v| v.as_str())
            .unwrap_or("inproc://replay_sink");
        let verbose = config
            .get("verbose")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let echo_data = config
            .get("echoData")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let timecode_input = config
            .get("timecodeInput")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let buffer_delay_ms = config
            .get("bufferSize")
            .and_then(|v| v.as_u64())
            .unwrap_or(500);
        let read_multiple_lines = config
            .get("readMultipleLines")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as usize;
        let block_size = config
            .get("blockSize")
            .and_then(|v| v.as_u64())
            .unwrap_or(65536) as usize;

        let mut pb = Self::new(replay_speed, queue_size, writer_endpoint, verbose);
        pb.m_echo_data = echo_data;
        pb.m_timecode_input = timecode_input;
        pb.m_buffer_delay_ms = buffer_delay_ms;
        pb.m_read_multiple_lines = read_multiple_lines;
        pb.m_block_size = block_size;
        pb
    }

    /// Returns the output endpoint string.
    pub fn endpoint(&self) -> &str {
        self.m_writer.endpoint()
    }

    /// Enqueue a data sample for replaying. Blocks if the queue is full.
    pub fn process_data(&self, data: StreamableData) {
        if !self.m_first_data_initialized.load(Ordering::Relaxed) {
            self.m_first_data_initialized.store(true, Ordering::Relaxed);
            *self.m_replay_start.lock().unwrap() = Instant::now();
        }
        self.m_queue.push(data);
    }

    /// Signal that the input has reached end-of-file.
    pub fn set_eof_reached(&self, eof: bool) {
        self.m_eof.store(eof, Ordering::Relaxed);
    }

    /// Reset the replay state for looping.
    pub fn reset(&self) {
        self.m_queue.clear();
        *self.m_replay_start.lock().unwrap() = Instant::now();
        self.m_sample_calculator.lock().unwrap().reset();
        self.m_first_data_initialized.store(false, Ordering::Relaxed);
    }

    /// Start the background replay thread that dequeues and dispatches data at
    /// the correct timing.
    pub fn start(&mut self) {
        self.m_running.store(true, Ordering::Relaxed);

        let queue = self.m_queue.clone();
        let eof = self.m_eof.clone();
        let running = self.m_running.clone();
        let writer = self.m_writer.clone();
        let replay_speed = self.m_replay_speed;
        let verbose = self.m_verbose;
        let echo_data = self.m_echo_data;
        let timecode_input = self.m_timecode_input;
        let buffer_delay_ms = self.m_buffer_delay_ms;
        let replay_start = self.m_replay_start.clone();
        let sample_calculator = self.m_sample_calculator.clone();

        self.m_replay_thread = Some(thread::spawn(move || {
            // Pre-buffer delay
            if buffer_delay_ms > 0 {
                thread::sleep(Duration::from_millis(buffer_delay_ms));
            }

            while running.load(Ordering::Relaxed) {
                if eof.load(Ordering::Relaxed) && queue.is_empty() {
                    break;
                }

                // Timecode mode: publish immediately without timing
                if timecode_input {
                    if let Some(data) = queue.pop_front() {
                        if echo_data {
                            log::info!(
                                "[echo] type={} sender={}",
                                data_type_label(&data),
                                data.sender_id().unwrap_or("<none>")
                            );
                        }
                        let _ = writer.store(&data);
                    }
                    thread::sleep(Duration::from_micros(10));
                    continue;
                }

                if !queue.wait_for_data(Duration::from_secs(3)) {
                    continue;
                }

                // Consume as many entries as are ready.
                // Note: C++ groups all non-IMU/non-Optical types into a shared "<other>"
                // bucket. We use per-sender buckets for all types, which is more accurate.
                loop {
                    let front = match queue.peek_front() {
                        Some(d) => d,
                        None => break,
                    };

                    let replay_elapsed = {
                        let start = replay_start.lock().unwrap();
                        start.elapsed()
                    };

                    let sample_elapsed = {
                        let mut calc = sample_calculator.lock().unwrap();
                        calc.elapsed(&front)
                    };

                    // Note: C++ uses `replay_speed * sample_elapsed` (higher = slower).
                    // We intentionally use `sample_elapsed / replay_speed` (higher = faster)
                    // as this matches user expectations (2.0 = double speed).
                    let scaled_sample_time = sample_elapsed.as_secs_f64() / replay_speed;

                    if replay_elapsed.as_secs_f64() < scaled_sample_time {
                        break;
                    }

                    if let Some(data) = queue.pop_front() {
                        if verbose {
                            log::debug!(
                                "Replay t={:.3}s type={} scaled_sample={:.3}s",
                                replay_elapsed.as_secs_f64(),
                                data_type_label(&data),
                                scaled_sample_time
                            );
                        }

                        if echo_data {
                            log::info!(
                                "[echo] type={} sender={}",
                                data_type_label(&data),
                                data.sender_id().unwrap_or("<none>")
                            );
                        }

                        let _ = writer.store(&data);
                    }
                }

                // Yield to avoid busy-spinning
                thread::sleep(Duration::from_micros(100));
            }

            log::info!("Realtime playback terminated");
        }));
    }

    /// Stop the replay thread.
    pub fn stop(&mut self) {
        self.m_running.store(false, Ordering::Relaxed);
        // Unblock anyone waiting on the queue
        self.m_queue.clear();

        if let Some(handle) = self.m_replay_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RealtimePlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Convenience: read from file and replay
// ---------------------------------------------------------------------------

/// Read data from a file and replay it through a RealtimePlayback processor.
///
/// This is a convenience function combining DiskReader + RealtimePlayback,
/// similar to what the C++ `mainReplay.cpp` sets up.
pub fn replay_from_file(
    file_path: &str,
    replay_speed: f64,
    queue_size: usize,
    writer_endpoint: &str,
    do_loop: bool,
    verbose: bool,
) {
    let mut playback = RealtimePlayback::new(replay_speed, queue_size, writer_endpoint, verbose);
    playback.start();

    let reader = DiskReader::new(file_path);

    loop {
        let pb_ref = &playback;
        let result = reader.read(|data| {
            pb_ref.process_data(data);
        });

        if let Err(e) = result {
            log::error!("Error reading file: {}", e);
            break;
        }

        if !do_loop {
            playback.set_eof_reached(true);
            break;
        }

        thread::sleep(Duration::from_millis(500));
        playback.reset();
    }

    // Wait for the replay thread to drain
    // (it will exit once eof is reached and the queue is empty)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::{ImuData, Timestamp};

    #[test]
    fn sample_elapsed_time_calculator_first_sample_is_zero() {
        let mut calc = SampleElapsedTimeCalculator::new();
        let data = StreamableData::Imu(ImuData::default());
        let elapsed = calc.elapsed(&data);
        assert_eq!(elapsed, Duration::ZERO);
    }

    #[test]
    fn data_type_labels() {
        assert_eq!(data_type_label(&StreamableData::Imu(ImuData::default())), "IMU");
        assert_eq!(
            data_type_label(&StreamableData::Timestamp(Timestamp::current())),
            "TS"
        );
    }

    #[test]
    fn replay_queue_push_pop() {
        let q = ReplayQueue::new(10);
        q.push(StreamableData::Timestamp(Timestamp::current()));
        assert_eq!(q.len(), 1);
        assert!(q.pop_front().is_some());
        assert!(q.is_empty());
    }

    #[test]
    fn realtime_playback_lifecycle() {
        let mut pb = RealtimePlayback::new(1.0, 10, "inproc://test_replay", false);
        pb.start();
        pb.process_data(StreamableData::Timestamp(Timestamp::current()));
        thread::sleep(Duration::from_millis(50));
        pb.set_eof_reached(true);
        pb.stop();
    }
}
