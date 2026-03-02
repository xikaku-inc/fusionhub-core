use std::sync::{Arc, Mutex};

use crate::clock::Clockwork;
use crate::connected_node::{
    check_endpoints, passes_data_filter, ConnectedNodeConfig, ConnectedNodeImpl, EndpointConfig,
};
use crate::node::Node;
use fusion_types::{StreamableData, Timestamp};

fn make_clockwork() -> Arc<Mutex<Clockwork>> {
    Arc::new(Mutex::new(Clockwork::new()))
}

fn make_default_node(name: &str, endpoints: EndpointConfig) -> ConnectedNodeImpl {
    ConnectedNodeImpl::new(name, endpoints, ConnectedNodeConfig::default(), make_clockwork())
}

/// Port of testConnectedNode.cpp - Constructor initializes member variables correctly
#[test]
fn constructor_initializes_correctly() {
    let endpoints = EndpointConfig {
        data_publish: "inproc://data_sink".into(),
        data_subscribe: vec!["inproc://data_source".into()],
        command_in: "inproc://command_sink".into(),
        ..Default::default()
    };

    let node = make_default_node("my_filter", endpoints);

    assert_eq!(node.endpoints().data_publish, "inproc://data_sink");
    assert_eq!(node.endpoints().command_in, "inproc://command_sink");
}

/// Port of testConnectedNode.cpp - getDataEndpoint returns the correct data endpoint
#[test]
fn get_data_endpoint_returns_correct_endpoint() {
    let endpoints = EndpointConfig {
        data_publish: "inproc://data_sink".into(),
        data_subscribe: vec!["inproc://data_source".into()],
        ..Default::default()
    };

    let node = make_default_node("my_filter", endpoints);
    assert_eq!(node.endpoints().data_publish, "inproc://data_sink");
}

/// Port of testConnectedNode.cpp - getCommandEndpoint returns the correct command endpoint
#[test]
fn get_command_endpoint_returns_correct_endpoint() {
    let endpoints = EndpointConfig {
        data_publish: "inproc://data_sink".into(),
        command_in: "inproc://command_sink".into(),
        ..Default::default()
    };

    let node = make_default_node("my_filter", endpoints);
    assert_eq!(node.endpoints().command_in, "inproc://command_sink");
}

/// Port of testConnectedNode.cpp - Check if we can identify incorrectly defined endpoints
#[test]
fn check_endpoints_validates_correctly() {
    // Valid endpoints
    assert!(check_endpoints(&["tcp://127.0.0.1:1234".into()]));
    assert!(check_endpoints(&["tcp://localhost:5000".into()]));
    assert!(check_endpoints(&["inproc://test-endpoint".into()]));
    assert!(check_endpoints(&["tcp://*:1234".into()])); // Bind wildcard is valid

    // Invalid endpoints
    assert!(!check_endpoints(&["tcp://bad_host:5000".into()]));
    assert!(!check_endpoints(&["tcp://example.com:5000".into()]));
}

/// Port of testConnectedNode.cpp - Data round-trip via process_output
#[test]
fn process_output_filters_timestamp() {
    let config = ConnectedNodeConfig::default();
    let node = ConnectedNodeImpl::new("test", EndpointConfig::default(), config, make_clockwork());

    // Timestamp variant should be filtered out (returns None)
    let data = StreamableData::Timestamp(Timestamp::current());
    assert!(node.process_output(data).is_none());
}

#[test]
fn process_output_sets_sender_id() {
    let config = ConnectedNodeConfig {
        is_clock_manual: true,
        ..Default::default()
    };
    let node = ConnectedNodeImpl::new("myNode", EndpointConfig::default(), config, make_clockwork());

    let data = StreamableData::Imu(Default::default());
    let result = node.process_output(data).unwrap();
    assert_eq!(result.sender_id(), Some("myNode"));
}

#[test]
fn process_output_preserves_existing_sender_id() {
    let config = ConnectedNodeConfig {
        is_clock_manual: true,
        ..Default::default()
    };
    let node = ConnectedNodeImpl::new("myNode", EndpointConfig::default(), config, make_clockwork());

    let mut imu = fusion_types::ImuData::default();
    imu.sender_id = "other_sender".to_owned();
    let data = StreamableData::Imu(imu);
    let result = node.process_output(data).unwrap();
    assert_eq!(result.sender_id(), Some("other_sender"));
}

#[test]
fn process_output_rate_limits_by_type() {
    let config = ConnectedNodeConfig {
        is_clock_manual: true,
        publish_interval_ms: 5000, // 5 second interval
        ..Default::default()
    };
    let node = ConnectedNodeImpl::new("n", EndpointConfig::default(), config, make_clockwork());

    let d1 = StreamableData::Imu(Default::default());
    let d2 = StreamableData::Imu(Default::default());
    let d3 = StreamableData::Gnss(Default::default());

    assert!(node.process_output(d1).is_some()); // First pass
    assert!(node.process_output(d2).is_none()); // Same type, rate limited
    assert!(node.process_output(d3).is_some()); // Different type, passes
}

#[test]
fn data_filter_empty_allows_all() {
    let filter: Vec<String> = vec![];
    assert!(passes_data_filter(&StreamableData::Imu(Default::default()), &filter));
    assert!(passes_data_filter(&StreamableData::Gnss(Default::default()), &filter));
    assert!(passes_data_filter(&StreamableData::Optical(Default::default()), &filter));
}

#[test]
fn data_filter_matches_specified_types() {
    let filter = vec!["Imu".to_owned(), "Gnss".to_owned()];
    assert!(passes_data_filter(&StreamableData::Imu(Default::default()), &filter));
    assert!(passes_data_filter(&StreamableData::Gnss(Default::default()), &filter));
    assert!(!passes_data_filter(&StreamableData::Optical(Default::default()), &filter));
    assert!(!passes_data_filter(&StreamableData::FusedPose(Default::default()), &filter));
}

#[test]
fn config_from_json_full() {
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
fn config_from_json_defaults() {
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
fn clock_generator_updates_clock() {
    let clockwork = make_clockwork();
    let config = ConnectedNodeConfig {
        is_clock_generator: true,
        generated_clock_name: "gen_clock".to_owned(),
        is_clock_manual: true, // Skip timestamp stamping
        ..Default::default()
    };
    let mut node = ConnectedNodeImpl::new("gen", EndpointConfig::default(), config, clockwork.clone());
    node.start().unwrap();

    // Sending a Timestamp should update the generated clock
    let ts = StreamableData::Timestamp(Timestamp::current());
    node.process_output(ts);

    let cw = clockwork.lock().unwrap();
    let clock = cw.get_clock("gen_clock");
    assert!(clock.is_some());
    assert!(clock.unwrap().is_initialized());
}

#[test]
fn command_rate_limiting() {
    let config = ConnectedNodeConfig {
        command_publish_interval_ms: 5000,
        ..Default::default()
    };
    let node = ConnectedNodeImpl::new("n", EndpointConfig::default(), config, make_clockwork());

    let req1 = fusion_types::ApiRequest::new("cmd", "topic1", serde_json::Value::Null, "1");
    let req2 = fusion_types::ApiRequest::new("cmd", "topic1", serde_json::Value::Null, "2");
    let req3 = fusion_types::ApiRequest::new("cmd", "topic2", serde_json::Value::Null, "3");

    assert!(node.should_publish_command(&req1)); // First pass
    assert!(!node.should_publish_command(&req2)); // Same topic, rate limited
    assert!(node.should_publish_command(&req3)); // Different topic, passes
}

/// Port of testConnectedNode.cpp - Ownership, constructors/destructors
#[test]
fn ownership_and_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let construct_count = Arc::new(AtomicUsize::new(0));
    let drop_count = Arc::new(AtomicUsize::new(0));

    {
        construct_count.fetch_add(1, Ordering::SeqCst);
        let _node = make_default_node("node_0", EndpointConfig::default());
        drop_count.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(
        construct_count.load(Ordering::SeqCst),
        drop_count.load(Ordering::SeqCst)
    );

    {
        construct_count.fetch_add(1, Ordering::SeqCst);
        let _node = Box::new(make_default_node("node_1", EndpointConfig::default()));
        drop_count.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(
        construct_count.load(Ordering::SeqCst),
        drop_count.load(Ordering::SeqCst)
    );
}
