use crate::encoders::json_encoder::{JsonDecoder, JsonEncoder};
use fusion_types::{
    ImuData, OpticalData, Quatd, StreamableData, Vec3d,
};
use std::time::{Duration, UNIX_EPOCH};

/// Port of testJsonEncoder.cpp - ImuData round trip
#[test]
fn imu_data_roundtrip() {
    let imu = ImuData {
        sender_id: "test_imu".into(),
        timestamp: UNIX_EPOCH + Duration::from_secs(1000),
        latency: 0.5,
        gyroscope: Vec3d::new(1.0, 2.0, 3.0),
        accelerometer: Vec3d::new(0.1, 0.2, 9.81),
        quaternion: Quatd::identity(),
        euler: Vec3d::new(10.0, 20.0, 30.0),
        period: 0.01,
        internal_frame_count: 42,
        linear_velocity: Vec3d::new(0.5, 0.0, 0.0),
    };

    let data = StreamableData::Imu(imu.clone());
    let json = JsonEncoder::encode(&data).unwrap();

    // Verify JSON is valid and uses C++ wrapper format
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v.get("imuData").is_some());

    let decoded = JsonDecoder::decode(&json).unwrap();
    match decoded {
        StreamableData::Imu(rimu) => {
            assert_eq!(rimu.sender_id, imu.sender_id);
            assert_eq!(rimu.timestamp, imu.timestamp);
            assert!((rimu.gyroscope - imu.gyroscope).norm() < 1e-10);
            assert!((rimu.accelerometer - imu.accelerometer).norm() < 1e-10);
            assert!(rimu.quaternion.angle_to(&imu.quaternion) < 1e-10);
            // Skipped fields get defaults after round-trip
            assert_eq!(rimu.latency, 0.0);
            assert_eq!(rimu.period, 0.0);
            assert_eq!(rimu.internal_frame_count, 0);
        }
        _ => panic!("Wrong variant after decode: expected Imu"),
    }
}

/// Port of testJsonEncoder.cpp - OpticalData round trip
#[test]
fn optical_data_roundtrip() {
    let optical = OpticalData {
        sender_id: "tracker".into(),
        timestamp: UNIX_EPOCH + Duration::from_secs(2000),
        last_data_time: UNIX_EPOCH + Duration::from_secs(1999),
        latency: 1.2,
        position: Vec3d::new(1.7, 0.0, 0.0),
        orientation: Quatd::identity(),
        angular_velocity: Vec3d::new(0.0, 0.0, 1.0),
        quality: 0.95,
        frame_rate: 90.0,
        frame_number: 100,
        interval: Duration::from_micros(11_111),
    };

    let data = StreamableData::Optical(optical.clone());
    let json = JsonEncoder::encode(&data).unwrap();

    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v.get("opticalData").is_some());

    let decoded = JsonDecoder::decode(&json).unwrap();
    match decoded {
        StreamableData::Optical(ropt) => {
            assert_eq!(ropt.sender_id, optical.sender_id);
            assert_eq!(ropt.timestamp, optical.timestamp);
            assert!((ropt.position - optical.position).norm() < 1e-10);
            assert!(ropt.orientation.angle_to(&optical.orientation) < 1e-10);
            // Skipped fields get defaults
            assert_eq!(ropt.latency, 0.0);
            assert_eq!(ropt.quality, 0.0);
            assert_eq!(ropt.frame_number, 0);
        }
        _ => panic!("Wrong variant after decode: expected Optical"),
    }
}

/// Test that encoding default ImuData produces valid JSON and decodes correctly
#[test]
fn imu_default_roundtrip() {
    let data = StreamableData::Imu(ImuData::default());
    let json = JsonEncoder::encode(&data).unwrap();
    let decoded = JsonDecoder::decode(&json).unwrap();
    match decoded {
        StreamableData::Imu(imu) => {
            assert_eq!(imu.period, 0.0);
            assert_eq!(imu.sender_id, "");
        }
        _ => panic!("Wrong variant"),
    }
}

/// Test that decoding an unknown type returns an error
#[test]
fn decode_unknown_type_returns_error() {
    let json = r#"{"unknownType": {}}"#;
    assert!(JsonDecoder::decode(json).is_err());
}

/// Test pretty encoding produces valid JSON
#[test]
fn pretty_encode_produces_valid_json() {
    let data = StreamableData::Imu(ImuData::default());
    let json = JsonEncoder::encode_pretty(&data).unwrap();
    let _: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(json.contains('\n'));
}
