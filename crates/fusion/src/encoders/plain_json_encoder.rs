use anyhow::{Context, Result};
use fusion_types::{encode_extension_json, StreamableData};

/// Encodes StreamableData directly as the inner data object, without a type wrapper.
/// Useful for external consumers that expect a flat JSON object.
pub struct PlainJsonEncoder;

impl PlainJsonEncoder {
    /// Encode StreamableData as a plain JSON object (no type tag).
    pub fn encode(data: &StreamableData) -> Result<String> {
        match data {
            StreamableData::Imu(d) => {
                serde_json::to_string(d).context("Failed to encode ImuData")
            }
            StreamableData::Gnss(d) => {
                serde_json::to_string(d).context("Failed to encode GnssData")
            }
            StreamableData::Optical(d) => {
                serde_json::to_string(d).context("Failed to encode OpticalData")
            }
            StreamableData::FusedPose(d) => {
                serde_json::to_string(d).context("Failed to encode FusedPose")
            }
            StreamableData::FusedVehiclePose(d) => {
                serde_json::to_string(d).context("Failed to encode FusedVehiclePose")
            }
            StreamableData::FusedVehiclePoseV2(d) => {
                serde_json::to_string(d).context("Failed to encode FusedVehiclePoseV2")
            }
            StreamableData::GlobalFusedPose(d) => {
                serde_json::to_string(d).context("Failed to encode GlobalFusedPose")
            }
            StreamableData::FusionStateInt(d) => {
                serde_json::to_string(d).context("Failed to encode FusionStateInt")
            }
            StreamableData::Rtcm(d) => {
                serde_json::to_string(d).context("Failed to encode RTCMData")
            }
            StreamableData::Can(d) => {
                serde_json::to_string(d).context("Failed to encode CANData")
            }
            StreamableData::VehicleState(d) => {
                serde_json::to_string(d).context("Failed to encode VehicleState")
            }
            StreamableData::VehicleSpeed(d) => {
                serde_json::to_string(d).context("Failed to encode VehicleSpeed")
            }
            StreamableData::VelocityMeter(d) => {
                serde_json::to_string(d).context("Failed to encode VelocityMeterData")
            }
            StreamableData::Timestamp(d) => {
                serde_json::to_string(d).context("Failed to encode Timestamp")
            }
            StreamableData::Extension(e) => {
                let payload = encode_extension_json(&e.type_name, e.payload_any())
                    .unwrap_or(serde_json::Value::Null);
                serde_json::to_string(&payload).context("Failed to encode Extension")
            }
        }
    }

    /// Encode StreamableData as a pretty-printed plain JSON object.
    pub fn encode_pretty(data: &StreamableData) -> Result<String> {
        match data {
            StreamableData::Imu(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::Gnss(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::Optical(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::FusedPose(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::FusedVehiclePose(d) => {
                serde_json::to_string_pretty(d).context("encode")
            }
            StreamableData::FusedVehiclePoseV2(d) => {
                serde_json::to_string_pretty(d).context("encode")
            }
            StreamableData::GlobalFusedPose(d) => {
                serde_json::to_string_pretty(d).context("encode")
            }
            StreamableData::FusionStateInt(d) => {
                serde_json::to_string_pretty(d).context("encode")
            }
            StreamableData::Rtcm(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::Can(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::VehicleState(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::VehicleSpeed(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::VelocityMeter(d) => {
                serde_json::to_string_pretty(d).context("encode")
            }
            StreamableData::Timestamp(d) => serde_json::to_string_pretty(d).context("encode"),
            StreamableData::Extension(e) => {
                let payload = encode_extension_json(&e.type_name, e.payload_any())
                    .unwrap_or(serde_json::Value::Null);
                serde_json::to_string_pretty(&payload).context("encode")
            }
        }
    }

    /// Return the type name of the variant.
    pub fn type_name(data: &StreamableData) -> &'static str {
        match data {
            StreamableData::Imu(_) => "ImuData",
            StreamableData::Gnss(_) => "GnssData",
            StreamableData::Optical(_) => "OpticalData",
            StreamableData::FusedPose(_) => "FusedPose",
            StreamableData::FusedVehiclePose(_) => "FusedVehiclePose",
            StreamableData::FusedVehiclePoseV2(_) => "FusedVehiclePoseV2",
            StreamableData::GlobalFusedPose(_) => "GlobalFusedPose",
            StreamableData::FusionStateInt(_) => "FusionStateInt",
            StreamableData::Rtcm(_) => "RTCMData",
            StreamableData::Can(_) => "CANData",
            StreamableData::VehicleState(_) => "VehicleState",
            StreamableData::VehicleSpeed(_) => "VehicleSpeed",
            StreamableData::VelocityMeter(_) => "VelocityMeterData",
            StreamableData::Timestamp(_) => "Timestamp",
            StreamableData::Extension(_) => "Extension",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::ImuData;

    #[test]
    fn plain_encode_imu() {
        let data = StreamableData::Imu(ImuData::default());
        let json = PlainJsonEncoder::encode(&data).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("senderId").is_some());
        assert!(parsed.get("gyroscope").is_some());
    }

    #[test]
    fn type_name_correct() {
        assert_eq!(
            PlainJsonEncoder::type_name(&StreamableData::Imu(ImuData::default())),
            "ImuData"
        );
    }
}
