use fusion_protobuf::ProtobufEncoder;
use fusion_types::{ImuData, OpticalData, Quatd, StreamableData, Vec3d};
use std::time::{Duration, UNIX_EPOCH};

/// Port of testProtobufEncoder.cpp - ImuData round trip
#[test]
fn imu_data_protobuf_roundtrip() {
    let imu = ImuData {
        sender_id: "test_imu".into(),
        timestamp: UNIX_EPOCH + Duration::from_secs(1000),
        latency: 0.5,
        gyroscope: Vec3d::new(1.0, 2.0, 3.0),
        accelerometer: Vec3d::new(0.1, 0.2, 9.81),
        quaternion: Quatd::identity(),
        euler: Vec3d::zeros(),
        period: 0.01,
        internal_frame_count: 42,
        linear_velocity: Vec3d::zeros(),
    };

    let encoder = ProtobufEncoder::new();
    let data = StreamableData::Imu(imu.clone());
    let encoded = encoder.encode(&data);
    assert!(!encoded.is_empty());

    let decoded = fusion_protobuf::decode(&encoded);
    assert!(decoded.is_some());

    match decoded.unwrap() {
        StreamableData::Imu(rimu) => {
            assert_eq!(rimu.sender_id, imu.sender_id);
            assert_eq!(rimu.timestamp, imu.timestamp);
            assert!((rimu.latency - imu.latency).abs() < 1e-10);
            assert!((rimu.gyroscope - imu.gyroscope).norm() < 1e-10);
            assert!((rimu.accelerometer - imu.accelerometer).norm() < 1e-10);
            assert!((rimu.period - imu.period).abs() < 1e-10);
            assert_eq!(rimu.internal_frame_count, imu.internal_frame_count);
        }
        _ => panic!("Wrong variant after decode: expected Imu"),
    }
}

/// Port of testProtobufEncoder.cpp - OpticalData round trip
#[test]
fn optical_data_protobuf_roundtrip() {
    let optical = OpticalData {
        sender_id: "tracker".into(),
        timestamp: UNIX_EPOCH + Duration::from_secs(2000),
        last_data_time: UNIX_EPOCH + Duration::from_secs(1999),
        latency: 1.2,
        position: Vec3d::new(1.7, 0.0, 0.0),
        orientation: Quatd::identity(),
        angular_velocity: Vec3d::zeros(),
        quality: 0.95,
        frame_rate: 90.0,
        frame_number: 100,
        interval: Duration::from_micros(11_111),
    };

    let encoder = ProtobufEncoder::new();
    let data = StreamableData::Optical(optical.clone());
    let encoded = encoder.encode(&data);
    assert!(!encoded.is_empty());

    let decoded = fusion_protobuf::decode(&encoded);
    assert!(decoded.is_some());

    match decoded.unwrap() {
        StreamableData::Optical(ropt) => {
            assert_eq!(ropt.sender_id, optical.sender_id);
            assert_eq!(ropt.timestamp, optical.timestamp);
            assert!((ropt.latency - optical.latency).abs() < 1e-10);
            assert!((ropt.position - optical.position).norm() < 1e-10);
            assert!(ropt.orientation.angle_to(&optical.orientation) < 1e-10);
            assert_eq!(ropt.frame_number, optical.frame_number);
            assert!((ropt.frame_rate - optical.frame_rate).abs() < 1e-10);
        }
        _ => panic!("Wrong variant after decode: expected Optical"),
    }
}

/// Test that encoding and decoding default values works
#[test]
fn default_imu_protobuf_roundtrip() {
    let encoder = ProtobufEncoder::new();
    let data = StreamableData::Imu(ImuData::default());
    let encoded = encoder.encode(&data);
    let decoded = fusion_protobuf::decode(&encoded);
    assert!(decoded.is_some());
    match decoded.unwrap() {
        StreamableData::Imu(imu) => {
            assert_eq!(imu.period, 0.0);
            assert_eq!(imu.sender_id, "");
        }
        _ => panic!("Wrong variant"),
    }
}

/// Test that decoding garbage bytes returns None
#[test]
fn decode_garbage_returns_none() {
    let garbage = b"this is not valid encoded data";
    let decoded = fusion_protobuf::decode(garbage);
    assert!(decoded.is_none());
}
