use anyhow::{Context, Result};
use fusion_types::StreamableData;

/// Serialize StreamableData to a JSON string with a C++-compatible wrapper.
/// The output is `{"<typeName>": <serialized>}` where typeName matches the C++ key.
pub struct JsonEncoder;

impl JsonEncoder {
    /// Encode StreamableData into a JSON string.
    /// The output matches the C++ JsonEncoder format:
    /// `{"imuData": {...}}`, `{"fusedPose": {...}}`, etc.
    pub fn encode(data: &StreamableData) -> Result<String> {
        let (wrapper_key, inner) = match data {
            StreamableData::Imu(d) => ("imuData", serde_json::to_value(d)?),
            StreamableData::Gnss(d) => ("gnssData", serde_json::to_value(d)?),
            StreamableData::Optical(d) => ("opticalData", serde_json::to_value(d)?),
            StreamableData::FusedPose(d) => ("fusedPose", serde_json::to_value(d)?),
            StreamableData::FusedVehiclePose(d) => ("fusedVehiclePose", serde_json::to_value(d)?),
            StreamableData::FusedVehiclePoseV2(d) => {
                ("fusedVehiclePoseV2", serde_json::to_value(d)?)
            }
            StreamableData::GlobalFusedPose(d) => ("globalFusedPose", serde_json::to_value(d)?),
            StreamableData::FusionStateInt(d) => ("FusionStateInt", serde_json::to_value(d)?),
            StreamableData::Rtcm(d) => ("RTCMData", serde_json::to_value(d)?),
            StreamableData::Can(d) => ("CANData", serde_json::to_value(d)?),
            StreamableData::VehicleState(d) => ("VehicleState", serde_json::to_value(d)?),
            StreamableData::VehicleSpeed(d) => ("VehicleSpeed", serde_json::to_value(d)?),
            StreamableData::VelocityMeter(d) => ("VelocityMeterData", serde_json::to_value(d)?),
            StreamableData::Timestamp(d) => ("Timestamp", serde_json::to_value(d)?),
        };
        let wrapper = serde_json::json!({ wrapper_key: inner });
        serde_json::to_string(&wrapper).context("Failed to encode StreamableData to JSON")
    }

    /// Encode StreamableData to a pretty-printed JSON string.
    pub fn encode_pretty(data: &StreamableData) -> Result<String> {
        let (wrapper_key, inner) = match data {
            StreamableData::Imu(d) => ("imuData", serde_json::to_value(d)?),
            StreamableData::Gnss(d) => ("gnssData", serde_json::to_value(d)?),
            StreamableData::Optical(d) => ("opticalData", serde_json::to_value(d)?),
            StreamableData::FusedPose(d) => ("fusedPose", serde_json::to_value(d)?),
            StreamableData::FusedVehiclePose(d) => ("fusedVehiclePose", serde_json::to_value(d)?),
            StreamableData::FusedVehiclePoseV2(d) => {
                ("fusedVehiclePoseV2", serde_json::to_value(d)?)
            }
            StreamableData::GlobalFusedPose(d) => ("globalFusedPose", serde_json::to_value(d)?),
            StreamableData::FusionStateInt(d) => ("FusionStateInt", serde_json::to_value(d)?),
            StreamableData::Rtcm(d) => ("RTCMData", serde_json::to_value(d)?),
            StreamableData::Can(d) => ("CANData", serde_json::to_value(d)?),
            StreamableData::VehicleState(d) => ("VehicleState", serde_json::to_value(d)?),
            StreamableData::VehicleSpeed(d) => ("VehicleSpeed", serde_json::to_value(d)?),
            StreamableData::VelocityMeter(d) => ("VelocityMeterData", serde_json::to_value(d)?),
            StreamableData::Timestamp(d) => ("Timestamp", serde_json::to_value(d)?),
        };
        let wrapper = serde_json::json!({ wrapper_key: inner });
        serde_json::to_string_pretty(&wrapper)
            .context("Failed to encode StreamableData to pretty JSON")
    }
}

/// Decode a JSON string back into StreamableData.
pub struct JsonDecoder;

impl JsonDecoder {
    /// Decode from the C++-compatible wrapper format: `{"<typeName>": {...}}`.
    /// The type is identified by the single top-level key.
    pub fn decode(json_str: &str) -> Result<StreamableData> {
        let wrapper: serde_json::Value =
            serde_json::from_str(json_str).context("Invalid JSON string")?;

        let obj = wrapper
            .as_object()
            .context("Expected JSON object at top level")?;

        for (key, data) in obj.iter() {
            return match key.as_str() {
                "imuData" => Ok(StreamableData::Imu(serde_json::from_value(data.clone())?)),
                "gnssData" => Ok(StreamableData::Gnss(serde_json::from_value(data.clone())?)),
                "opticalData" => Ok(StreamableData::Optical(serde_json::from_value(
                    data.clone(),
                )?)),
                "fusedPose" => Ok(StreamableData::FusedPose(serde_json::from_value(
                    data.clone(),
                )?)),
                "fusedVehiclePose" => Ok(StreamableData::FusedVehiclePose(
                    serde_json::from_value(data.clone())?,
                )),
                "fusedVehiclePoseV2" => Ok(StreamableData::FusedVehiclePoseV2(
                    serde_json::from_value(data.clone())?,
                )),
                "globalFusedPose" => Ok(StreamableData::GlobalFusedPose(
                    serde_json::from_value(data.clone())?,
                )),
                "FusionStateInt" => Ok(StreamableData::FusionStateInt(serde_json::from_value(
                    data.clone(),
                )?)),
                "RTCMData" => Ok(StreamableData::Rtcm(serde_json::from_value(data.clone())?)),
                "CANData" => Ok(StreamableData::Can(serde_json::from_value(data.clone())?)),
                "VehicleState" => Ok(StreamableData::VehicleState(serde_json::from_value(
                    data.clone(),
                )?)),
                "VehicleSpeed" => Ok(StreamableData::VehicleSpeed(serde_json::from_value(
                    data.clone(),
                )?)),
                "VelocityMeterData" => Ok(StreamableData::VelocityMeter(
                    serde_json::from_value(data.clone())?,
                )),
                "Timestamp" => Ok(StreamableData::Timestamp(serde_json::from_value(
                    data.clone(),
                )?)),
                _ => anyhow::bail!("Unknown StreamableData type key: '{}'", key),
            };
        }

        anyhow::bail!("Empty JSON object, no type key found")
    }

    /// Try to decode StreamableData directly (serde tagged enum format).
    pub fn decode_direct(json_str: &str) -> Result<StreamableData> {
        serde_json::from_str(json_str).context("Failed to decode StreamableData from JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::{FusedPose, ImuData, Timestamp, Vec3d};

    #[test]
    fn roundtrip_imu() {
        let data = StreamableData::Imu(ImuData::default());
        let json = JsonEncoder::encode(&data).unwrap();
        let decoded = JsonDecoder::decode(&json).unwrap();
        match decoded {
            StreamableData::Imu(imu) => {
                assert_eq!(imu.period, 0.0);
            }
            _ => panic!("Wrong variant after decode"),
        }
    }

    #[test]
    fn roundtrip_timestamp() {
        let data = StreamableData::Timestamp(Timestamp::current());
        let json = JsonEncoder::encode(&data).unwrap();
        let decoded = JsonDecoder::decode(&json).unwrap();
        assert!(matches!(decoded, StreamableData::Timestamp(_)));
    }

    #[test]
    fn decode_unknown_type() {
        let json = r#"{"unknownType": {}}"#;
        assert!(JsonDecoder::decode(json).is_err());
    }

    #[test]
    fn encode_fused_pose_cpp_format() {
        let data = StreamableData::FusedPose(FusedPose {
            sender_id: "test".into(),
            acceleration: Vec3d::new(1.0, 2.0, 3.0),
            ..Default::default()
        });
        let json = JsonEncoder::encode(&data).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("fusedPose").is_some(), "should have fusedPose key");
        assert!(v.get("type").is_none(), "should NOT have type key");
        assert!(v.get("data").is_none(), "should NOT have data key");
        let inner = &v["fusedPose"];
        assert_eq!(inner["senderId"].as_str().unwrap(), "test");
        assert_eq!(inner["acceleration"]["x"].as_f64().unwrap(), 1.0);
    }

    #[test]
    fn decode_cpp_fused_pose() {
        let cpp_json = r#"{"fusedPose":{"acceleration":{"x":-0.00010963330856322145,"y":0.0002469858365108813,"z":0.0001497355501149933},"lastDataTime":0,"orientation":{"w":0.009526392636495448,"x":0.009362874074581006,"y":0.7085831102307157,"z":0.705500928651525},"position":{"x":0.0,"y":0.0,"z":0.0},"senderId":"/sinks/fusion/settings","timestamp":1772137433109455200,"transmissionTime":0,"velocity":{"x":0.0,"y":0.0,"z":0.0}}}"#;
        let decoded = JsonDecoder::decode(cpp_json).unwrap();
        match decoded {
            StreamableData::FusedPose(fp) => {
                assert_eq!(fp.sender_id, "/sinks/fusion/settings");
                assert!((fp.orientation.w - 0.009526392636495448).abs() < 1e-10);
            }
            _ => panic!("Expected FusedPose variant"),
        }
    }

    #[test]
    fn decode_cpp_imu_data() {
        let cpp_json = r#"{"imuData":{"timestamp":1000000000,"senderId":"imu0","gyroscope":{"x":1.0,"y":2.0,"z":3.0},"accelerometer":{"x":0.1,"y":0.2,"z":9.81},"quaternion":{"w":1.0,"x":0.0,"y":0.0,"z":0.0},"euler":{"x":0.0,"y":0.0,"z":0.0}}}"#;
        let decoded = JsonDecoder::decode(cpp_json).unwrap();
        match decoded {
            StreamableData::Imu(imu) => {
                assert_eq!(imu.sender_id, "imu0");
                assert_eq!(imu.gyroscope.x, 1.0);
                assert_eq!(imu.accelerometer.z, 9.81);
            }
            _ => panic!("Expected Imu variant"),
        }
    }

    #[test]
    fn roundtrip_all_types() {
        use fusion_types::*;
        let types: Vec<StreamableData> = vec![
            StreamableData::Imu(ImuData::default()),
            StreamableData::Gnss(GnssData::default()),
            StreamableData::Optical(OpticalData::default()),
            StreamableData::FusedPose(FusedPose::default()),
            StreamableData::FusedVehiclePose(FusedVehiclePose::default()),
            StreamableData::FusedVehiclePoseV2(FusedVehiclePoseV2::default()),
            StreamableData::GlobalFusedPose(GlobalFusedPose::default()),
            StreamableData::FusionStateInt(FusionStateInt::default()),
            StreamableData::Rtcm(RTCMData::default()),
            StreamableData::Can(CANData::default()),
            StreamableData::VehicleState(VehicleState::default()),
            StreamableData::VehicleSpeed(VehicleSpeed::default()),
            StreamableData::VelocityMeter(VelocityMeterData::default()),
            StreamableData::Timestamp(Timestamp::default()),
        ];
        for original in &types {
            let json = JsonEncoder::encode(original).unwrap();
            let decoded = JsonDecoder::decode(&json).unwrap();
            assert_eq!(original.variant_name(), decoded.variant_name());
        }
    }
}
