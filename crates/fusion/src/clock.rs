use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A named clock with offset tracking relative to system time.
#[derive(Clone, Debug)]
pub struct Clock {
    m_name: String,
    m_offset: Duration,
    m_offset_sign: bool,
    m_last_update: SystemTime,
    m_initialized: bool,
}

impl Clock {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            m_name: name.into(),
            m_offset: Duration::ZERO,
            m_offset_sign: true,
            m_last_update: UNIX_EPOCH,
            m_initialized: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.m_name
    }

    pub fn is_initialized(&self) -> bool {
        self.m_initialized
    }

    /// Set clock offset from an external timestamp.
    pub fn set_offset_from_timestamp(&mut self, external_time: SystemTime) {
        let now = SystemTime::now();
        match external_time.duration_since(now) {
            Ok(diff) => {
                self.m_offset = diff;
                self.m_offset_sign = true;
            }
            Err(e) => {
                self.m_offset = e.duration();
                self.m_offset_sign = false;
            }
        }
        self.m_last_update = now;
        self.m_initialized = true;
    }

    /// Get the current time according to this clock.
    pub fn now(&self) -> SystemTime {
        let sys_now = SystemTime::now();
        if self.m_offset_sign {
            sys_now + self.m_offset
        } else {
            sys_now - self.m_offset
        }
    }

    /// Get offset as signed seconds.
    pub fn offset_secs(&self) -> f64 {
        let secs = self.m_offset.as_secs_f64();
        if self.m_offset_sign {
            secs
        } else {
            -secs
        }
    }

    pub fn last_update(&self) -> SystemTime {
        self.m_last_update
    }
}

/// Timecode source that maps external timecodes to system time.
#[derive(Clone, Debug)]
pub struct TimecodeSource {
    m_name: String,
    m_rate: f64,
    m_last_timecode: f64,
    m_last_system_time: SystemTime,
    m_initialized: bool,
}

impl TimecodeSource {
    pub fn new(name: impl Into<String>, rate: f64) -> Self {
        Self {
            m_name: name.into(),
            m_rate: rate,
            m_last_timecode: 0.0,
            m_last_system_time: UNIX_EPOCH,
            m_initialized: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.m_name
    }

    pub fn rate(&self) -> f64 {
        self.m_rate
    }

    pub fn update(&mut self, timecode: f64) {
        self.m_last_timecode = timecode;
        self.m_last_system_time = SystemTime::now();
        self.m_initialized = true;
    }

    pub fn is_initialized(&self) -> bool {
        self.m_initialized
    }

    /// Convert a timecode value to a SystemTime.
    pub fn timecode_to_system_time(&self, timecode: f64) -> SystemTime {
        if !self.m_initialized {
            return SystemTime::now();
        }
        let dt = (timecode - self.m_last_timecode) / self.m_rate;
        if dt >= 0.0 {
            self.m_last_system_time + Duration::from_secs_f64(dt)
        } else {
            self.m_last_system_time - Duration::from_secs_f64(-dt)
        }
    }
}

/// A tick-based clock that increments by a fixed interval per tick.
#[derive(Clone, Debug)]
pub struct TickClock {
    m_name: String,
    m_tick_interval: Duration,
    m_ticks: u64,
    m_base_time: SystemTime,
}

impl TickClock {
    pub fn new(name: impl Into<String>, tick_interval: Duration) -> Self {
        Self {
            m_name: name.into(),
            m_tick_interval: tick_interval,
            m_ticks: 0,
            m_base_time: SystemTime::now(),
        }
    }

    pub fn tick(&mut self) {
        self.m_ticks += 1;
    }

    pub fn reset(&mut self) {
        self.m_ticks = 0;
        self.m_base_time = SystemTime::now();
    }

    pub fn now(&self) -> SystemTime {
        self.m_base_time + self.m_tick_interval * self.m_ticks as u32
    }

    pub fn ticks(&self) -> u64 {
        self.m_ticks
    }

    pub fn name(&self) -> &str {
        &self.m_name
    }
}

/// Manages multiple named clocks and selects the active one.
#[derive(Clone, Debug)]
pub struct Clockwork {
    m_clocks: HashMap<String, Clock>,
    m_active_clock: Option<String>,
}

impl Clockwork {
    pub fn new() -> Self {
        Self {
            m_clocks: HashMap::new(),
            m_active_clock: None,
        }
    }

    pub fn add_clock(&mut self, clock: Clock) {
        let name = clock.name().to_owned();
        self.m_clocks.insert(name.clone(), clock);
        if self.m_active_clock.is_none() {
            self.m_active_clock = Some(name);
        }
    }

    pub fn get_clock(&self, name: &str) -> Option<&Clock> {
        self.m_clocks.get(name)
    }

    pub fn get_clock_mut(&mut self, name: &str) -> Option<&mut Clock> {
        self.m_clocks.get_mut(name)
    }

    pub fn set_active(&mut self, name: &str) -> bool {
        if self.m_clocks.contains_key(name) {
            self.m_active_clock = Some(name.to_owned());
            true
        } else {
            false
        }
    }

    pub fn active_clock(&self) -> Option<&Clock> {
        self.m_active_clock
            .as_ref()
            .and_then(|name| self.m_clocks.get(name))
    }

    /// Get current time from the active clock, or system time if none.
    pub fn now(&self) -> SystemTime {
        match self.active_clock() {
            Some(clock) if clock.is_initialized() => clock.now(),
            _ => SystemTime::now(),
        }
    }

    /// Update a clock from an incoming data timestamp.
    pub fn update_clock(&mut self, clock_name: &str, external_time: SystemTime) {
        if let Some(clock) = self.m_clocks.get_mut(clock_name) {
            clock.set_offset_from_timestamp(external_time);
        } else {
            let mut clock = Clock::new(clock_name);
            clock.set_offset_from_timestamp(external_time);
            self.m_clocks.insert(clock_name.to_owned(), clock);
        }
    }

    pub fn clock_names(&self) -> Vec<&str> {
        self.m_clocks.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for Clockwork {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_offset() {
        let mut clock = Clock::new("test");
        assert!(!clock.is_initialized());
        let future = SystemTime::now() + Duration::from_secs(10);
        clock.set_offset_from_timestamp(future);
        assert!(clock.is_initialized());
        assert!(clock.offset_secs() > 9.0 && clock.offset_secs() < 11.0);
    }

    #[test]
    fn clockwork_multiple_clocks() {
        let mut cw = Clockwork::new();
        cw.add_clock(Clock::new("gps"));
        cw.add_clock(Clock::new("imu"));
        assert_eq!(cw.clock_names().len(), 2);
        assert!(cw.active_clock().is_some());
    }

    #[test]
    fn tick_clock() {
        let mut tc = TickClock::new("tc", Duration::from_millis(10));
        let t0 = tc.now();
        tc.tick();
        tc.tick();
        let t2 = tc.now();
        let diff = t2.duration_since(t0).unwrap();
        assert_eq!(diff.as_millis(), 20);
    }

    #[test]
    fn timecode_source() {
        let mut tcs = TimecodeSource::new("ltc", 30.0);
        tcs.update(100.0);
        assert!(tcs.is_initialized());
        let t = tcs.timecode_to_system_time(101.0);
        let now = SystemTime::now();
        let diff = t.duration_since(now).unwrap_or(Duration::ZERO);
        assert!(diff.as_millis() < 100);
    }
}
