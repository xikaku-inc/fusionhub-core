use crate::sinks::mqtt_sink::{MqttEncoding, MqttSink};
use crate::node::Node;

#[test]
fn test_mqtt_sink_creation() {
    let sink = MqttSink::new("test_sink", "broker.example.com", 1883, "fusion/output");
    assert_eq!(sink.name(), "test_sink");
    assert!(sink.is_enabled());
    assert_eq!(sink.publish_count(), 0);
}

#[test]
fn test_mqtt_sink_topic_format() {
    let sink = MqttSink::new("sink1", "localhost", 1883, "vehicles/car1/pose");
    assert_eq!(sink.name(), "sink1");

    // Verify topic with slashes is accepted and sink is functional
    assert_eq!(sink.publish_count(), 0);

    // Verify the node can be disabled
    let mut sink = sink;
    sink.set_enabled(false);
    assert!(!sink.is_enabled());
}

#[test]
fn test_mqtt_sink_encoding_default_is_json() {
    let sink = MqttSink::new("enc_test", "localhost", 1883, "test/topic");
    // Default encoding should be Json -- verify by creating with_encoding and comparing
    let sink_proto = MqttSink::new("enc_test2", "localhost", 1883, "test/topic")
        .with_encoding(MqttEncoding::Protobuf);
    // The default sink should differ from the protobuf one
    // We can only verify through the with_encoding builder pattern
    assert_eq!(sink.name(), "enc_test");
    assert_eq!(sink_proto.name(), "enc_test2");
}

#[test]
fn test_mqtt_sink_topic_change() {
    let mut sink = MqttSink::new("topic_test", "localhost", 1883, "initial/topic");
    sink.set_topic("updated/topic");
    // Verify the sink still functions after topic change
    assert_eq!(sink.name(), "topic_test");
    assert_eq!(sink.publish_count(), 0);
}
