use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fusion_types::{ApiRequest, JsonValueExt, StreamableData};

use crate::clock::Clockwork;
use crate::command_router::CommandRouter;
use crate::node::{Node, NodeBase};

/// Trait for nodes that participate in network pub/sub.
pub trait ConnectedNode: Node {
    fn data_endpoint(&self) -> &str;
    fn command_endpoint(&self) -> &str;
    fn clockwork(&self) -> &Clockwork;
}

/// Configuration for network endpoints.
#[derive(Clone, Debug, Default)]
pub struct EndpointConfig {
    pub data_publish: String,
    pub data_subscribe: Vec<String>,
    pub command_in: String,
    pub command_out: String,
    pub command_input_endpoints: Vec<String>,
}

/// Configuration extracted from JSON for ConnectedNode features.
/// Mirrors C++ ConnectedNode fields: clocks, rate limiting, data filtering.
#[derive(Clone, Debug)]
pub struct ConnectedNodeConfig {
    pub data_clock_name: String,
    pub is_clock_generator: bool,
    pub is_clock_manual: bool,
    pub generated_clock_name: String,
    pub publish_interval_ms: u64,
    pub command_publish_interval_ms: u64,
    pub input_data_filter: Vec<String>,
}

impl Default for ConnectedNodeConfig {
    fn default() -> Self {
        Self {
            data_clock_name: "system_clock".to_owned(),
            is_clock_generator: false,
            is_clock_manual: false,
            generated_clock_name: String::new(),
            publish_interval_ms: 0,
            command_publish_interval_ms: 100,
            input_data_filter: Vec::new(),
        }
    }
}

impl ConnectedNodeConfig {
    /// Parse connected node config from JSON, mirroring C++ ConnectedNode constructor.
    pub fn from_json(config: &serde_json::Value, node_name: &str) -> Self {
        let data_clock_name = config.value_str("dataClockName", "system_clock");
        let is_clock_generator = config.value_bool("isClockGenerator", false);
        let is_clock_manual = config.value_bool("isClockManual", false);

        let default_clock_name = format!("{}_clock", node_name);
        let generated_clock_name = config.value_str("generatedClockName", &default_clock_name);

        let publish_interval_ms = config.value_u64("publishIntervalMs", 0);
        let command_publish_interval_ms = config.value_u64("commandPublishIntervalMs", 100);

        let input_data_filter = config
            .get("inputDataFilter")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            data_clock_name,
            is_clock_generator,
            is_clock_manual,
            generated_clock_name,
            publish_interval_ms,
            command_publish_interval_ms,
            input_data_filter,
        }
    }
}

/// Rate limiter for data publishing (per variant type) and command publishing (per topic).
/// Thread-safe — used from output callbacks which may run on different threads.
pub struct OutputRateLimiter {
    m_publish_interval_ms: u64,
    m_command_publish_interval_ms: u64,
    m_last_data_publish: Mutex<HashMap<&'static str, Instant>>,
    m_last_command_publish: Mutex<HashMap<String, Instant>>,
}

impl OutputRateLimiter {
    pub fn new(publish_interval_ms: u64, command_publish_interval_ms: u64) -> Self {
        Self {
            m_publish_interval_ms: publish_interval_ms,
            m_command_publish_interval_ms: command_publish_interval_ms,
            m_last_data_publish: Mutex::new(HashMap::new()),
            m_last_command_publish: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if this data should be published (not rate-limited).
    pub fn should_publish_data(&self, data: &StreamableData) -> bool {
        if self.m_publish_interval_ms == 0 {
            return true;
        }

        let variant_name = data.variant_name();
        let now = Instant::now();
        let interval = std::time::Duration::from_millis(self.m_publish_interval_ms);

        let mut map = self.m_last_data_publish.lock().unwrap();
        match map.get(variant_name) {
            Some(last) if now.duration_since(*last) < interval => false,
            _ => {
                map.insert(variant_name, now);
                true
            }
        }
    }

    /// Returns true if this command should be published (not rate-limited).
    pub fn should_publish_command(&self, topic: &str) -> bool {
        if self.m_command_publish_interval_ms == 0 {
            return true;
        }

        let now = Instant::now();
        let interval = std::time::Duration::from_millis(self.m_command_publish_interval_ms);

        let mut map = self.m_last_command_publish.lock().unwrap();
        match map.get(topic) {
            Some(last) if now.duration_since(*last) < interval => false,
            _ => {
                map.insert(topic.to_owned(), now);
                true
            }
        }
    }
}

/// Checks whether an input data item passes the data type filter.
/// An empty filter means all data passes.
pub fn passes_data_filter(data: &StreamableData, filter: &[String]) -> bool {
    if filter.is_empty() {
        return true;
    }
    filter.iter().any(|f| f == data.variant_name())
}

/// Validate endpoints (mirroring C++ ConnectedNode::checkEndpoints).
/// Returns true if all endpoints appear valid.
pub fn check_endpoints(endpoints: &[String]) -> bool {
    for ep in endpoints {
        if ep.contains("inproc://") {
            continue;
        }

        let addr = if let Some(stripped) = ep.strip_prefix("tcp://") {
            stripped
        } else {
            ep.as_str()
        };

        if !is_valid_ip_or_localhost_with_port(addr) {
            log::warn!("Invalid endpoint definition: {}", ep);
            return false;
        }
    }

    true
}

/// Check if an address matches "IP:port" or "localhost:port".
fn is_valid_ip_or_localhost_with_port(addr: &str) -> bool {
    let (host, port_str) = match addr.rsplit_once(':') {
        Some(pair) => pair,
        None => return false,
    };

    // Port must be numeric
    if port_str.is_empty() || !port_str.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    if host == "localhost" || host == "*" {
        return true;
    }

    // Must be IPv4: d.d.d.d
    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() != 4 {
        return false;
    }

    for octet in &octets {
        if octet.is_empty() || octet.len() > 3 || !octet.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if let Ok(n) = octet.parse::<u16>() {
            if n > 255 {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

/// Wraps a Node with network pub/sub capabilities.
/// Publishes node output via protobuf over ZMQ and subscribes to input endpoints.
pub struct ConnectedNodeImpl {
    pub base: NodeBase,
    m_endpoints: EndpointConfig,
    m_config: ConnectedNodeConfig,
    m_clockwork: Arc<Mutex<Clockwork>>,
    m_command_router: CommandRouter,
    m_rate_limiter: Arc<OutputRateLimiter>,
    m_inactive_once: Mutex<bool>,
    m_input_count: AtomicU64,
    m_output_count: AtomicU64,
}

impl ConnectedNodeImpl {
    pub fn new(
        name: impl Into<String>,
        endpoints: EndpointConfig,
        config: ConnectedNodeConfig,
        clockwork: Arc<Mutex<Clockwork>>,
    ) -> Self {
        let name = name.into();
        let rate_limiter = Arc::new(OutputRateLimiter::new(
            config.publish_interval_ms,
            config.command_publish_interval_ms,
        ));

        Self {
            base: NodeBase::new(&name),
            m_endpoints: endpoints,
            m_config: config,
            m_clockwork: clockwork,
            m_command_router: CommandRouter::new(&name),
            m_rate_limiter: rate_limiter,
            m_inactive_once: Mutex::new(true),
            m_input_count: AtomicU64::new(0),
            m_output_count: AtomicU64::new(0),
        }
    }

    pub fn endpoints(&self) -> &EndpointConfig {
        &self.m_endpoints
    }

    pub fn config(&self) -> &ConnectedNodeConfig {
        &self.m_config
    }

    pub fn command_router(&self) -> &CommandRouter {
        &self.m_command_router
    }

    pub fn command_router_mut(&mut self) -> &mut CommandRouter {
        &mut self.m_command_router
    }

    pub fn clockwork(&self) -> &Arc<Mutex<Clockwork>> {
        &self.m_clockwork
    }

    pub fn rate_limiter(&self) -> &Arc<OutputRateLimiter> {
        &self.m_rate_limiter
    }

    pub fn input_data_filter(&self) -> &[String] {
        &self.m_config.input_data_filter
    }

    pub fn input_count(&self) -> u64 {
        self.m_input_count.load(Ordering::Relaxed)
    }

    pub fn output_count(&self) -> u64 {
        self.m_output_count.load(Ordering::Relaxed)
    }

    pub fn increment_input(&self) {
        self.m_input_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_output(&self) {
        self.m_output_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Prepare clocks matching C++ ConnectedNode::prepareClocks.
    pub fn prepare_clocks(&mut self) {
        let mut cw = self.m_clockwork.lock().unwrap();

        if self.m_config.is_clock_generator {
            // Try to add the generated clock
            let clock = crate::clock::Clock::new(&self.m_config.generated_clock_name);
            cw.add_clock(clock);
            // Force data to use the generated clock
            self.m_config.data_clock_name = self.m_config.generated_clock_name.clone();
            log::info!(
                "[{}] Clock generator: '{}'",
                self.base.name(),
                self.m_config.generated_clock_name
            );
        }

        if self.m_config.is_clock_manual {
            log::info!(
                "[{}] Clock set to manual — node is responsible for its own timestamps",
                self.base.name()
            );
        }

        if !self.m_config.input_data_filter.is_empty() {
            log::info!(
                "[{}] Input data filter: {:?}",
                self.base.name(),
                self.m_config.input_data_filter
            );
        }
    }

    /// Process outgoing data: set sender ID, stamp timestamp, rate-limit.
    /// Mirrors the C++ data consumer callback in ConnectedNode::initializeConnections.
    /// Returns Some(data) if it should be published, None if rate-limited or is a Timestamp variant.
    pub fn process_output(&self, mut data: StreamableData) -> Option<StreamableData> {
        // Handle Timestamp variant: update generated clock if applicable
        if data.is_timestamp() {
            if self.m_config.is_clock_generator {
                if let Some(ts) = data.timestamp() {
                    self.m_clockwork.lock().unwrap().update_clock(
                        &self.m_config.generated_clock_name,
                        ts,
                    );
                }
            }
            // Timestamps are not published to the network (matching C++ Void check)
            return None;
        }

        // Auto-fill sender ID if empty
        if data.sender_id().map_or(false, |id| id.is_empty()) {
            data.set_sender_id(self.base.name());
        }

        // Stamp timestamp from clock (unless manual mode)
        if !self.m_config.is_clock_manual {
            let cw = self.m_clockwork.lock().unwrap();
            if let Some(clock) = cw.get_clock(&self.m_config.data_clock_name) {
                if clock.is_initialized() {
                    data.set_timestamp(clock.now());
                } else {
                    // C++ behavior: publish data without updating timestamp when clock is inactive
                    // (don't block output)
                    let mut inactive_once = self.m_inactive_once.lock().unwrap();
                    if *inactive_once {
                        *inactive_once = false;
                        log::info!(
                            "[{}] Subscribed data clock '{}' still inactive. Publishing data with original timestamp.",
                            self.base.name(),
                            self.m_config.data_clock_name
                        );
                    }
                }
            }
            // If clock not found, use system time (default behavior)
        }

        // Rate limiting per data type
        if !self.m_rate_limiter.should_publish_data(&data) {
            return None;
        }

        Some(data)
    }

    /// Process outgoing command: rate-limit by topic.
    /// Returns true if the command should be published.
    pub fn should_publish_command(&self, request: &ApiRequest) -> bool {
        self.m_rate_limiter.should_publish_command(&request.topic)
    }

    /// Handle an incoming API command.
    pub fn handle_command(&self, request: &ApiRequest) -> Option<ApiRequest> {
        self.m_command_router.route(request)
    }
}

impl Node for ConnectedNodeImpl {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.prepare_clocks();
        log::info!(
            "ConnectedNode '{}' starting (pub={}, sub={:?})",
            self.base.name(),
            self.m_endpoints.data_publish,
            self.m_endpoints.data_subscribe
        );
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("ConnectedNode '{}' stopping", self.base.name());
        self.base.stop_heartbeat();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::Timestamp;

    fn make_clockwork() -> Arc<Mutex<Clockwork>> {
        Arc::new(Mutex::new(Clockwork::new()))
    }

    #[test]
    fn connected_node_basic() {
        let endpoints = EndpointConfig {
            data_publish: "tcp://127.0.0.1:5000".into(),
            data_subscribe: vec!["tcp://127.0.0.1:5001".into()],
            ..Default::default()
        };
        let config = ConnectedNodeConfig::default();
        let mut node = ConnectedNodeImpl::new("test_connected", endpoints, config, make_clockwork());
        assert_eq!(node.endpoints().data_publish, "tcp://127.0.0.1:5000");
        assert!(node.start().is_ok());
        assert!(node.stop().is_ok());
    }

    #[test]
    fn config_from_json() {
        let json = serde_json::json!({
            "dataClockName": "gps_clock",
            "isClockGenerator": true,
            "generatedClockName": "my_clock",
            "isClockManual": false,
            "publishIntervalMs": 50,
            "commandPublishIntervalMs": 200,
            "inputDataFilter": ["Imu", "Gnss"]
        });

        let config = ConnectedNodeConfig::from_json(&json, "testNode");
        assert_eq!(config.data_clock_name, "gps_clock");
        assert!(config.is_clock_generator);
        assert_eq!(config.generated_clock_name, "my_clock");
        assert!(!config.is_clock_manual);
        assert_eq!(config.publish_interval_ms, 50);
        assert_eq!(config.command_publish_interval_ms, 200);
        assert_eq!(config.input_data_filter, vec!["Imu", "Gnss"]);
    }

    #[test]
    fn config_defaults() {
        let json = serde_json::json!({});
        let config = ConnectedNodeConfig::from_json(&json, "myNode");
        assert_eq!(config.data_clock_name, "system_clock");
        assert!(!config.is_clock_generator);
        assert_eq!(config.generated_clock_name, "myNode_clock");
        assert!(!config.is_clock_manual);
        assert_eq!(config.publish_interval_ms, 0);
        assert_eq!(config.command_publish_interval_ms, 100);
        assert!(config.input_data_filter.is_empty());
    }

    #[test]
    fn rate_limiter_no_limit() {
        let rl = OutputRateLimiter::new(0, 0);
        let data = StreamableData::Timestamp(Timestamp::current());
        assert!(rl.should_publish_data(&data));
        assert!(rl.should_publish_data(&data));
        assert!(rl.should_publish_command("topic"));
        assert!(rl.should_publish_command("topic"));
    }

    #[test]
    fn rate_limiter_limits_data() {
        let rl = OutputRateLimiter::new(1000, 0); // 1 second interval
        let data = StreamableData::Timestamp(Timestamp::current());
        assert!(rl.should_publish_data(&data)); // First always passes
        assert!(!rl.should_publish_data(&data)); // Second within interval → blocked
    }

    #[test]
    fn rate_limiter_different_types_independent() {
        let rl = OutputRateLimiter::new(1000, 0);
        let ts = StreamableData::Timestamp(Timestamp::current());
        let imu = StreamableData::Imu(Default::default());
        assert!(rl.should_publish_data(&ts));
        assert!(rl.should_publish_data(&imu)); // Different type → passes
        assert!(!rl.should_publish_data(&ts)); // Same type → blocked
        assert!(!rl.should_publish_data(&imu)); // Same type → blocked
    }

    #[test]
    fn rate_limiter_limits_commands() {
        let rl = OutputRateLimiter::new(0, 1000);
        assert!(rl.should_publish_command("topic1"));
        assert!(!rl.should_publish_command("topic1")); // Same topic → blocked
        assert!(rl.should_publish_command("topic2")); // Different topic → passes
    }

    #[test]
    fn data_filter_empty_passes_all() {
        let filter: Vec<String> = vec![];
        let data = StreamableData::Imu(Default::default());
        assert!(passes_data_filter(&data, &filter));
    }

    #[test]
    fn data_filter_matches() {
        let filter = vec!["Imu".to_owned(), "Gnss".to_owned()];
        assert!(passes_data_filter(&StreamableData::Imu(Default::default()), &filter));
        assert!(passes_data_filter(&StreamableData::Gnss(Default::default()), &filter));
        assert!(!passes_data_filter(&StreamableData::Optical(Default::default()), &filter));
    }

    #[test]
    fn process_output_sets_sender_id() {
        let config = ConnectedNodeConfig {
            is_clock_manual: true, // Skip clock logic for this test
            ..Default::default()
        };
        let node = ConnectedNodeImpl::new("myNode", EndpointConfig::default(), config, make_clockwork());
        let data = StreamableData::Imu(Default::default());
        let result = node.process_output(data).unwrap();
        assert_eq!(result.sender_id(), Some("myNode"));
    }

    #[test]
    fn process_output_timestamp_returns_none() {
        let config = ConnectedNodeConfig::default();
        let node = ConnectedNodeImpl::new("n", EndpointConfig::default(), config, make_clockwork());
        let data = StreamableData::Timestamp(Timestamp::current());
        assert!(node.process_output(data).is_none());
    }

    #[test]
    fn process_output_rate_limited() {
        let config = ConnectedNodeConfig {
            is_clock_manual: true,
            publish_interval_ms: 5000,
            ..Default::default()
        };
        let node = ConnectedNodeImpl::new("n", EndpointConfig::default(), config, make_clockwork());

        let d1 = StreamableData::Imu(Default::default());
        let d2 = StreamableData::Imu(Default::default());
        assert!(node.process_output(d1).is_some());
        assert!(node.process_output(d2).is_none()); // Rate limited
    }

    #[test]
    fn check_endpoints_valid() {
        assert!(check_endpoints(&["tcp://127.0.0.1:5000".into()]));
        assert!(check_endpoints(&["tcp://localhost:5000".into()]));
        assert!(check_endpoints(&["inproc://test".into()]));
    }

    #[test]
    fn check_endpoints_invalid() {
        assert!(!check_endpoints(&["tcp://bad_host:5000".into()]));
    }
}
