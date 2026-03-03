use std::sync::{Arc, Mutex};

use fusion_registry::{
    NodeMetadata, NodeRole, SettingsField,
    extract_settings,
};
use fusion_types::JsonValueExt;
use serde_json::json;

fn sf(key: &str, label: &str, ft: &str, default: serde_json::Value) -> SettingsField {
    SettingsField { key: key.into(), label: label.into(), field_type: ft.into(), default, options: None }
}

fn aliases(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

const SOURCE_COLOR: &str = "#4ade80";
const FILTER_COLOR: &str = "#6c8cff";
const SINK_COLOR: &str = "#f87171";

pub fn register_core_nodes() {
    // Example Source — emits Timestamp data at a configurable interval.
    fusion_registry::register_node(
        NodeMetadata {
            id: "exampleSource".into(),
            display_name: "Example Source".into(),
            description: None,
            role: NodeRole::Source,
            config_aliases: aliases(&["ExampleSource", "exampleSource"]),
            inputs: vec![],
            outputs: vec!["Timestamp".into()],
            default_settings: json!({"intervalMs": 1000}),
            settings_schema: vec![
                sf("intervalMs", "Interval (ms)", "number", json!(1000)),
            ],
            subtypes: None,
            required_feature: None,
            supports_realtime_config: false,
            color: SOURCE_COLOR.into(),
        },
        |type_name, config| {
            let settings = extract_settings(config);
            let name = config.value_str("name", type_name);
            let interval = settings.value_u64("intervalMs", 1000);
            Ok(Arc::new(Mutex::new(crate::sources::example_source::ExampleSource::new(&name, interval))))
        },
    );

    // Example Filter — passes through all received data unchanged.
    fusion_registry::register_node(
        NodeMetadata {
            id: "exampleFilter".into(),
            display_name: "Example Filter".into(),
            description: None,
            role: NodeRole::Filter,
            config_aliases: aliases(&["ExampleFilter", "exampleFilter"]),
            inputs: vec![],
            outputs: vec![],
            default_settings: json!({}),
            settings_schema: vec![],
            subtypes: None,
            required_feature: None,
            supports_realtime_config: false,
            color: FILTER_COLOR.into(),
        },
        |type_name, config| {
            let name = config.value_str("name", type_name);
            Ok(Arc::new(Mutex::new(crate::filters::example_filter::ExampleFilter::new(&name))))
        },
    );

    // Example Sink — counts received messages, logs at debug level.
    fusion_registry::register_node(
        NodeMetadata {
            id: "exampleSink".into(),
            display_name: "Example Sink".into(),
            description: None,
            role: NodeRole::Sink,
            config_aliases: aliases(&["ExampleSink", "exampleSink"]),
            inputs: vec![],
            outputs: vec![],
            default_settings: json!({}),
            settings_schema: vec![],
            subtypes: None,
            required_feature: None,
            supports_realtime_config: false,
            color: SINK_COLOR.into(),
        },
        |type_name, config| {
            let name = config.value_str("name", type_name);
            Ok(Arc::new(Mutex::new(crate::sinks::example_sink::ExampleSink::new(&name))))
        },
    );

    // Data Monitor (always-on internal sink)
    fusion_registry::register_node(
        NodeMetadata {
            id: "dataMonitor".into(),
            display_name: "Data Monitor".into(),
            description: Some("Internal monitoring sink that tracks data flow rates, sample counts, and quality metrics for all connected sources.".into()),
            role: NodeRole::Sink,
            config_aliases: aliases(&["DataMonitor", "dataMonitor"]),
            inputs: vec![],
            outputs: vec![],
            default_settings: json!({}),
            settings_schema: vec![],
            subtypes: None,
            required_feature: None,
            supports_realtime_config: false,
            color: SINK_COLOR.into(),
        },
        |type_name, config| {
            let name = config.value_str("name", type_name);
            Ok(Arc::new(Mutex::new(crate::sinks::data_monitor::DataMonitor::new(&name))))
        },
    );
}
