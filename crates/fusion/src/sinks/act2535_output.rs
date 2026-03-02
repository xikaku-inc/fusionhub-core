use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fusion_types::{StreamableData, VelocityMeterData};

use crate::node::{Node, NodeBase};

/// Format a velocity meter data line in the ACT2535 protocol format.
///
/// Output format: `V<velocity>,D<distance>,M<material>,L<level>,S<status>\r\n`
pub mod act2535_data_format {
    use fusion_types::VelocityMeterData;

    pub fn format_act2535_line(data: &VelocityMeterData) -> String {
        format!(
            "V{:.4},D{:.4},M{:.1},L{:.1},S{}\r\n",
            data.velocity, data.distance, data.material, data.doppler_level, data.output_status
        )
    }
}

/// Serial output sink for ACT2535 velocity meters.
/// Rate-limited to avoid overwhelming the serial device.
pub struct Act2535Output {
    pub base: NodeBase,
    m_port_name: String,
    m_baud_rate: u32,
    m_serial: Arc<Mutex<Option<Box<dyn serialport::SerialPort>>>>,
    m_last_send: Arc<Mutex<Instant>>,
    m_min_interval: Duration,
    m_count: Arc<Mutex<u64>>,
}

impl Act2535Output {
    pub fn new(name: impl Into<String>, port: &str, baud_rate: u32) -> Self {
        Self {
            base: NodeBase::new(name),
            m_port_name: port.to_owned(),
            m_baud_rate: baud_rate,
            m_serial: Arc::new(Mutex::new(None)),
            m_last_send: Arc::new(Mutex::new(Instant::now())),
            m_min_interval: Duration::from_millis(20), // 50 Hz max
            m_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn set_min_interval(&mut self, interval: Duration) {
        self.m_min_interval = interval;
    }

    pub fn on_data(&self, data: StreamableData) {
        if !self.base.is_enabled() {
            return;
        }

        if let StreamableData::VelocityMeter(ref vm) = data {
            self.send_velocity_data(vm);
        }

        self.base.notify_consumers(data);
    }

    fn send_velocity_data(&self, data: &VelocityMeterData) {
        let mut last_send = self.m_last_send.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(*last_send) < self.m_min_interval {
            return;
        }
        *last_send = now;
        drop(last_send);

        let line = act2535_data_format::format_act2535_line(data);

        let mut serial = self.m_serial.lock().unwrap();
        if let Some(ref mut port) = *serial {
            if let Err(e) = std::io::Write::write_all(port.as_mut(), line.as_bytes()) {
                log::warn!(
                    "[{}] Serial write failed: {}",
                    self.base.name(),
                    e
                );
                return;
            }
        }

        let mut count = self.m_count.lock().unwrap();
        *count += 1;
    }

    pub fn send_count(&self) -> u64 {
        *self.m_count.lock().unwrap()
    }
}

impl Node for Act2535Output {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Act2535Output '{}' starting (port={}, baud={})",
            self.base.name(),
            self.m_port_name,
            self.m_baud_rate
        );

        if self.m_port_name.is_empty() {
            log::warn!(
                "[{}] No serial port configured, output will be discarded",
                self.base.name()
            );
            return Ok(());
        }

        match serialport::new(&self.m_port_name, self.m_baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
        {
            Ok(port) => {
                *self.m_serial.lock().unwrap() = Some(port);
                log::info!(
                    "[{}] Serial port '{}' opened at {} baud",
                    self.base.name(),
                    self.m_port_name,
                    self.m_baud_rate
                );
            }
            Err(e) => {
                log::warn!(
                    "[{}] Could not open serial port '{}': {}. Node will run without output.",
                    self.base.name(),
                    self.m_port_name,
                    e
                );
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Act2535Output '{}' stopping (sent {} lines)",
            self.base.name(),
            self.send_count()
        );
        *self.m_serial.lock().unwrap() = None;
        self.base.stop_heartbeat();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }

    fn receive_data(&mut self, data: StreamableData) {
        self.on_data(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_act2535_line_basic() {
        let data = VelocityMeterData {
            velocity: 1.2345,
            distance: 100.5678,
            material: 3.0,
            doppler_level: 42.5,
            output_status: 1,
            ..Default::default()
        };
        let line = act2535_data_format::format_act2535_line(&data);
        assert!(line.starts_with("V1.2345"));
        assert!(line.contains("D100.5678"));
        assert!(line.contains("M3.0"));
        assert!(line.contains("L42.5"));
        assert!(line.contains("S1"));
        assert!(line.ends_with("\r\n"));
    }

    #[test]
    fn act2535_output_creation() {
        let output = Act2535Output::new("act_test", "COM3", 115200);
        assert_eq!(output.name(), "act_test");
        assert_eq!(output.send_count(), 0);
    }
}
