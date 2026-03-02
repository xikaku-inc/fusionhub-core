use crate::filters::prediction_filter::PredictionFilter;
use fusion_types::{ApiRequest, FusedPose, Quatd, StreamableData, Vec3d};
use std::sync::{Arc, Mutex};

/// Port of testPredictionFilter.cpp - Constructor sets node name and prediction interval
#[test]
fn constructor_sets_name_and_interval() {
    let config = serde_json::json!({
        "name": "filter",
        "rotationInterval": 0.1,
    });

    let filter = PredictionFilter::new(config);

    assert_eq!(filter.name(), "filter");
    assert!((filter.rotation_interval() - 0.1).abs() < 1e-12);
}

/// Port of testPredictionFilter.cpp - Default config has zero interval
#[test]
fn default_config_has_zero_interval() {
    let filter = PredictionFilter::new(serde_json::json!({}));
    assert_eq!(filter.rotation_interval(), 0.0);
}

/// Port of testPredictionFilter.cpp - ProcessData integrates orientation
#[test]
fn process_data_integrates_orientation() {
    let config = serde_json::json!({
        "name": "filter",
        "rotationInterval": 0.1,
    });

    let mut filter = PredictionFilter::new(config);

    let results = Arc::new(Mutex::new(Vec::new()));
    let r = results.clone();
    filter.set_on_output(Box::new(move |data| {
        r.lock().unwrap().push(data);
    }));

    let pose = FusedPose {
        orientation: Quatd::identity(),
        angular_velocity: Vec3d::new(0.0, 0.0, 90.0), // 90 deg/s around Z
        ..Default::default()
    };

    filter.process_data(StreamableData::FusedPose(pose));

    let output = results.lock().unwrap();
    assert_eq!(output.len(), 1);

    if let StreamableData::FusedPose(ref p) = output[0] {
        // 90 deg/s for 0.1s = 9 degrees = ~0.157 rad
        let angle = p.orientation.angle_to(&Quatd::identity());
        assert!(
            (angle - 0.157).abs() < 0.02,
            "Expected ~0.157 rad, got {} rad",
            angle
        );
    } else {
        panic!("Expected FusedPose output");
    }
}

/// Port of testPredictionFilter.cpp - Zero interval produces identity-like orientation
#[test]
fn zero_interval_produces_no_rotation() {
    let mut filter = PredictionFilter::new(serde_json::json!({"rotationInterval": 0.0}));

    let results = Arc::new(Mutex::new(Vec::new()));
    let r = results.clone();
    filter.set_on_output(Box::new(move |data| {
        r.lock().unwrap().push(data);
    }));

    let pose = FusedPose {
        orientation: Quatd::identity(),
        angular_velocity: Vec3d::new(0.0, 0.0, 90.0),
        ..Default::default()
    };

    filter.process_data(StreamableData::FusedPose(pose));

    let output = results.lock().unwrap();
    assert_eq!(output.len(), 1);

    if let StreamableData::FusedPose(ref p) = output[0] {
        let angle = p.orientation.angle_to(&Quatd::identity());
        assert!(angle < 1e-6, "Expected identity, got angle {} rad", angle);
    } else {
        panic!("Expected FusedPose output");
    }
}

/// Port of testPredictionFilter.cpp - ProcessCommand updates config (setConfigJsonPath)
#[test]
fn process_command_updates_config() {
    let mut filter = PredictionFilter::new(serde_json::json!({"name": "pred"}));
    assert_eq!(filter.rotation_interval(), 0.0);

    let cmd = ApiRequest::new(
        "setConfigJsonPath",
        "pred",
        serde_json::json!({"rotationInterval": 0.2}),
        "1",
    );

    filter.process_command(&cmd);

    assert!(
        (filter.rotation_interval() - 0.2).abs() < 1e-12,
        "Rotation interval should be updated to 0.2"
    );
}

/// Test that unhandled commands do not change state
#[test]
fn unhandled_command_is_ignored() {
    let mut filter =
        PredictionFilter::new(serde_json::json!({"rotationInterval": 0.05, "name": "pred"}));

    let cmd = ApiRequest::new(
        "unknownCommand",
        "pred",
        serde_json::json!({}),
        "1",
    );

    filter.process_command(&cmd);
    assert!((filter.rotation_interval() - 0.05).abs() < 1e-12);
}
