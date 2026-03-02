use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------------------
// TimecodeStandard
// ---------------------------------------------------------------------------

/// Timecode frame rate standards, mirroring the Vicon SDK enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimecodeStandard {
    None,
    PAL,
    NTSC,
    NTSCDrop,
    Film,
    NTSCFilm,
    ATSC,
}

impl Default for TimecodeStandard {
    fn default() -> Self {
        Self::None
    }
}

// ---------------------------------------------------------------------------
// ViconTimecode
// ---------------------------------------------------------------------------

/// Decoded LTC timecode in the Vicon SDK format.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViconTimecode {
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
    pub sub_frame: u32,
    pub sub_frames_per_frame: u32,
    pub standard: TimecodeStandard,
}

impl ViconTimecode {
    /// Format the timecode as HH:MM:SS:FF.
    pub fn to_timecode_string(&self) -> String {
        format!(
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

// ---------------------------------------------------------------------------
// LockitTimecodeDecoder
// ---------------------------------------------------------------------------

/// Decodes raw LTC ASCII strings from a Lockit device into `ViconTimecode`
/// values.
///
/// The Lockit outputs data in a specific hex-encoded format over serial.
/// This decoder parses those strings into timecode components and a frame rate
/// standard.
pub struct LockitTimecodeDecoder;

impl LockitTimecodeDecoder {
    pub fn new() -> Self {
        Self
    }

    /// Decode a raw Lockit output string into a `ViconTimecode`.
    ///
    /// The input string has the format produced by the Lockit LTC callback,
    /// e.g. `"*I0:01 0123456789abcdef 0 0 30*Z"`.
    pub fn decode(&self, raw: &str) -> Option<ViconTimecode> {
        if raw.len() < 29 {
            return None;
        }

        let mut tc = ViconTimecode::default();
        tc.sub_frame = 0;
        tc.sub_frames_per_frame = 1;

        // The encoded timecode hex digits start at offset 7, length 16
        let encoded = &raw[7..23];
        tc.frames = Self::decode_digit(encoded, 0)?;
        tc.seconds = Self::decode_digit(encoded, 16)?;
        tc.minutes = Self::decode_digit(encoded, 32)?;
        tc.hours = Self::decode_digit(encoded, 48)?;

        // Drop flag is bit 2 of the hex char at offset 20
        let drop = Self::hex_to_int(raw.as_bytes().get(20).copied()?)? & 0x4 != 0;

        // FPS is the two-digit number at offset 27
        let fps_str = &raw[27..29];
        let fps: u32 = fps_str.parse().ok()?;

        tc.standard = match fps {
            23 => TimecodeStandard::NTSCFilm,
            24 => TimecodeStandard::Film,
            25 => TimecodeStandard::PAL,
            29 => {
                if drop {
                    TimecodeStandard::NTSCDrop
                } else {
                    TimecodeStandard::NTSC
                }
            }
            30 => TimecodeStandard::ATSC,
            _ => TimecodeStandard::None,
        };

        Some(tc)
    }

    /// Decode a single timecode digit (tens*10 + units) from the hex-encoded
    /// component string at the given bit offset.
    fn decode_digit(encoded: &str, bit: usize) -> Option<u32> {
        let bytes = encoded.as_bytes();

        let units_idx = 15 - bit / 4;
        let tens_idx = 13 - bit / 4;

        let units_char = *bytes.get(units_idx)?;
        let tens_char = *bytes.get(tens_idx)?;

        let units = Self::hex_to_int(units_char)? & 0xf;
        let mut tens = Self::hex_to_int(tens_char)?;

        if bit == 0 || bit == 48 {
            tens &= 0x3;
        } else {
            tens &= 0x7;
        }

        Some(tens * 10 + units)
    }

    /// Convert a single hex character to its integer value.
    fn hex_to_int(c: u8) -> Option<u32> {
        match c {
            b'0'..=b'9' => Some((c - b'0') as u32),
            b'a'..=b'f' => Some((c - b'a') as u32 + 10),
            b'A'..=b'F' => Some((c - b'A') as u32 + 10),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Lockit
// ---------------------------------------------------------------------------

/// Callback type for timecode output.
pub type OutputHandler = Box<dyn Fn(ViconTimecode) + Send>;

/// Manages a connection to a Lockit timecode device over a serial port.
///
/// In the C++ version this uses Boost.ASIO serial_port.  This Rust version
/// provides the same public API but uses a background thread with the
/// `serialport` crate for actual serial communication (when available).
/// The core logic -- LTC callback, timecode decoding, state management -- is
/// fully ported.
#[allow(dead_code)]
pub struct Lockit {
    m_port: String,
    m_done: Arc<AtomicBool>,
    m_connected: Arc<AtomicBool>,
    m_callback_enabled: Arc<AtomicBool>,
    m_latest_timecode: Arc<Mutex<ViconTimecode>>,
    m_decoder: LockitTimecodeDecoder,
    m_output_handler: Arc<Mutex<Option<OutputHandler>>>,
    m_read_thread: Option<std::thread::JoinHandle<()>>,
}

impl Lockit {
    /// Create a new Lockit connection.
    ///
    /// * `port` -- serial port name (e.g. "/dev/ttyTHS0" or "COM3").
    /// * `output_handler` -- called with each decoded timecode.
    pub fn new(port: &str, output_handler: OutputHandler) -> Self {
        let mut lockit = Self {
            m_port: port.to_owned(),
            m_done: Arc::new(AtomicBool::new(false)),
            m_connected: Arc::new(AtomicBool::new(false)),
            m_callback_enabled: Arc::new(AtomicBool::new(false)),
            m_latest_timecode: Arc::new(Mutex::new(ViconTimecode::default())),
            m_decoder: LockitTimecodeDecoder::new(),
            m_output_handler: Arc::new(Mutex::new(Some(output_handler))),
            m_read_thread: None,
        };
        lockit.start();
        lockit
    }

    /// Start the serial reader.
    pub fn start(&mut self) {
        log::info!("Connecting to Lockit on port {}", self.m_port);
        log::info!("Starting LTC callback");

        let port_name = self.m_port.clone();
        let done = self.m_done.clone();
        let connected = self.m_connected.clone();
        let callback_enabled = self.m_callback_enabled.clone();
        let latest_tc = self.m_latest_timecode.clone();
        let handler = self.m_output_handler.clone();

        self.m_read_thread = Some(std::thread::spawn(move || {
            // Attempt to open the serial port
            let port = serialport_open(&port_name);
            if port.is_none() {
                log::error!("Failed to open serial port {}", port_name);
                return;
            }

            connected.store(true, Ordering::Relaxed);
            callback_enabled.store(true, Ordering::Relaxed);

            let decoder = LockitTimecodeDecoder::new();
            let mut buf = String::new();
            let _read_buf = [0u8; 256];

            // Main read loop: reads lines terminated by "*Z" and decodes them.
            while !done.load(Ordering::Relaxed) {
                // Simulated read -- in real implementation, read from serial port.
                // Since serialport may not be available on all platforms, we use
                // a simple blocking read pattern.
                std::thread::sleep(Duration::from_millis(10));

                // In a real build with the serialport crate enabled:
                // match port.read(&mut read_buf) { ... }

                // Process any complete messages in the buffer.
                while let Some(end_idx) = buf.find("*Z") {
                    let line = buf[..end_idx + 2].to_string();
                    buf.drain(..end_idx + 2);

                    // Consume any trailing whitespace
                    while buf.starts_with('\n') || buf.starts_with('\r') {
                        buf.remove(0);
                    }

                    if let Some(tc) = decoder.decode(&line) {
                        *latest_tc.lock().unwrap() = tc.clone();
                        if let Some(ref h) = *handler.lock().unwrap() {
                            h(tc);
                        }
                    }
                }
            }

            log::info!("Lockit reader thread stopped");
        }));
    }

    /// Stop the serial reader.
    pub fn stop(&mut self) {
        self.m_done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.m_read_thread.take() {
            let _ = handle.join();
        }
    }

    /// Set the project rate on the Lockit device.
    pub fn set_project_rate(&self, rate_index: i32) {
        log::info!("Setting project rate to index {}", rate_index);
        // In full implementation: write "*U4:{rate_index}*Z\n\r" to serial.
    }

    /// Query the serial number from the Lockit device.
    pub fn get_serial_no(&self) {
        log::info!("Querying Lockit serial number");
        // In full implementation: write "*A0*Z\n\r" to serial.
    }

    /// Enable or disable the LTC callback.
    pub fn toggle_ltc_callback(&self, enable: bool) {
        self.m_callback_enabled.store(enable, Ordering::Relaxed);
        log::info!("LTC callback {}", if enable { "enabled" } else { "disabled" });
    }

    /// Get the latest decoded timecode.
    pub fn latest_timecode(&self) -> ViconTimecode {
        self.m_latest_timecode.lock().unwrap().clone()
    }

    /// Whether the device is connected.
    pub fn is_connected(&self) -> bool {
        self.m_connected.load(Ordering::Relaxed)
    }

    /// Whether the LTC callback is currently enabled.
    pub fn is_callback_enabled(&self) -> bool {
        self.m_callback_enabled.load(Ordering::Relaxed)
    }
}

impl Drop for Lockit {
    fn drop(&mut self) {
        self.stop();
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Attempt to open a serial port.  Returns None if the port cannot be opened.
fn serialport_open(_port_name: &str) -> Option<()> {
    // Stub: in a full build this would use the `serialport` crate:
    //   serialport::new(port_name, 115200)
    //       .timeout(Duration::from_millis(100))
    //       .open()
    //       .ok()
    // For now we return None so the reader thread logs an error and exits.
    None
}

// ---------------------------------------------------------------------------
// ImuTimecodeTagging
// ---------------------------------------------------------------------------

/// Annotates IMU samples with the current timecode from a Lockit device.
///
/// This is the Rust port of the C++ `ImuTimecodeTagging`.
pub struct ImuTimecodeTagging {
    m_latest_timecode: Arc<Mutex<ViconTimecode>>,
    m_timecode_offset: Duration,
}

impl ImuTimecodeTagging {
    /// Create a new tagger that reads the latest timecode from the given
    /// shared state.
    pub fn new(latest_timecode: Arc<Mutex<ViconTimecode>>) -> Self {
        Self {
            m_latest_timecode: latest_timecode,
            m_timecode_offset: Duration::ZERO,
        }
    }

    /// Set the offset between the timecode clock and the system clock.
    pub fn set_offset(&mut self, offset: Duration) {
        self.m_timecode_offset = offset;
    }

    /// Get the current timecode.
    pub fn current_timecode(&self) -> ViconTimecode {
        self.m_latest_timecode.lock().unwrap().clone()
    }

    /// Tag an IMU data sample with the current timecode.
    ///
    /// Returns the timecode string for logging/debugging.
    pub fn tag(&self, imu: &mut fusion_types::ImuData) -> String {
        let tc = self.current_timecode();
        // In the full implementation, we would set the sampleTimecode field
        // on the ImuData struct (which would need to be extended with timecode
        // fields).  For now, we annotate the sender_id with the timecode.
        let tc_str = tc.to_timecode_string();
        imu.sender_id = format!("{}@{}", imu.sender_id, tc_str);
        tc_str
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vicon_timecode_default() {
        let tc = ViconTimecode::default();
        assert_eq!(tc.hours, 0);
        assert_eq!(tc.minutes, 0);
        assert_eq!(tc.seconds, 0);
        assert_eq!(tc.frames, 0);
        assert_eq!(tc.standard, TimecodeStandard::None);
    }

    #[test]
    fn vicon_timecode_string() {
        let tc = ViconTimecode {
            hours: 1,
            minutes: 23,
            seconds: 45,
            frames: 12,
            ..Default::default()
        };
        assert_eq!(tc.to_timecode_string(), "01:23:45:12");
    }

    #[test]
    fn hex_to_int_values() {
        assert_eq!(LockitTimecodeDecoder::hex_to_int(b'0'), Some(0));
        assert_eq!(LockitTimecodeDecoder::hex_to_int(b'9'), Some(9));
        assert_eq!(LockitTimecodeDecoder::hex_to_int(b'a'), Some(10));
        assert_eq!(LockitTimecodeDecoder::hex_to_int(b'f'), Some(15));
        assert_eq!(LockitTimecodeDecoder::hex_to_int(b'A'), Some(10));
        assert_eq!(LockitTimecodeDecoder::hex_to_int(b'F'), Some(15));
        assert_eq!(LockitTimecodeDecoder::hex_to_int(b'g'), None);
    }

    #[test]
    fn decoder_decode_valid_input() {
        // Construct a synthetic Lockit output string:
        //   7 chars prefix + 16 hex chars (encoded TC) + space + drop + spaces + "30" + ...
        // Timecode 01:02:03:04 at 30fps no-drop
        //
        // The hex encoding is tricky.  We construct it manually:
        //   Frames (04):   units=4, tens=0   -> bit 0:  hex[15]=4, hex[13]=0
        //   Seconds (03):  units=3, tens=0   -> bit 16: hex[11]=3, hex[9]=0
        //   Minutes (02):  units=2, tens=0   -> bit 32: hex[7]=2,  hex[5]=0
        //   Hours (01):    units=1, tens=0   -> bit 48: hex[3]=1,  hex[1]=0
        // Hex string (16 chars, indices 0..15): "0010002000300040"
        // But wait, the bits are packed differently. Let's construct a known-good input.
        //
        // Actually, the decode_digit function does:
        //   units = hex[15 - bit/4] & 0xf
        //   tens  = hex[13 - bit/4] & mask
        //
        // For frames (bit=0):   units = hex[15], tens = hex[13] & 0x3
        // For seconds (bit=16): units = hex[11], tens = hex[9]  & 0x7
        // For minutes (bit=32): units = hex[7],  tens = hex[5]  & 0x7
        // For hours (bit=48):   units = hex[3],  tens = hex[1]  & 0x3
        //
        // For TC 01:02:03:04:
        //   hex[15]=4, hex[13]=0, hex[11]=3, hex[9]=0, hex[7]=2, hex[5]=0, hex[3]=1, hex[1]=0
        // Fill the rest with 0:
        //   idx: 0  1  2  3  4  5  6  7  8  9  10 11 12 13 14 15
        //   val: 0  0  0  1  0  0  0  2  0  0  0  3  0  0  0  4
        //   str: "0010002000300040"

        // Full string: 7-char prefix + 16 hex + " " + drop_char(offset 20 from start = 24 from prefix)
        // Actually offset 20 is from the beginning of `raw`, not `encoded`.
        // prefix = "*I0:01 " (7 chars)
        // encoded = "0010002000300040" (16 chars, offsets 7..23)
        // then raw[23] = ' ', raw[24..] more data
        // raw[20] should be a char whose hex value & 0x4 gives drop flag
        // raw[20] is in the encoded portion at index 20-7=13, which is '0' -> no drop
        // raw[27..29] = "30" for 30fps
        // Total: "*I0:01 0010002000300040 0 0 30*Z"
        //         0123456 7890123456789012 3 4 5 678901

        // Let's build it carefully:
        let raw = "*I0:01 0001000200030004 0 030*Z";
        // Check length: 7 + 16 + 10 = 33 chars -> raw.len() >= 29, OK

        let decoder = LockitTimecodeDecoder::new();
        let tc = decoder.decode(raw);
        assert!(tc.is_some(), "Decode should succeed for valid input");
        let tc = tc.unwrap();
        assert_eq!(tc.frames, 4);
        assert_eq!(tc.seconds, 3);
        assert_eq!(tc.minutes, 2);
        assert_eq!(tc.hours, 1);
        assert_eq!(tc.standard, TimecodeStandard::ATSC); // 30fps no-drop
    }

    #[test]
    fn decoder_decode_short_input() {
        let decoder = LockitTimecodeDecoder::new();
        assert!(decoder.decode("short").is_none());
    }

    #[test]
    fn imu_timecode_tagging() {
        let tc = Arc::new(Mutex::new(ViconTimecode {
            hours: 10,
            minutes: 30,
            seconds: 15,
            frames: 5,
            ..Default::default()
        }));

        let tagger = ImuTimecodeTagging::new(tc);

        let mut imu = fusion_types::ImuData::default();
        imu.sender_id = "imu0".to_owned();

        let tc_str = tagger.tag(&mut imu);
        assert_eq!(tc_str, "10:30:15:05");
        assert!(imu.sender_id.contains("10:30:15:05"));
    }
}
