use std::any::Any;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use nalgebra::{Isometry3, Matrix3, Rotation3, UnitQuaternion, Vector2, Vector3};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// JsonValueExt -- ergonomic JSON value extraction (mirrors C++ nlohmann .value())
// ---------------------------------------------------------------------------

/// Extension trait on `serde_json::Value` that provides concise typed accessors
/// with defaults, mirroring C++ nlohmann::json's `.value("key", default)`.
///
/// ```
/// use serde_json::json;
/// use fusion_types::JsonValueExt;
///
/// let config = json!({"host": "localhost", "port": 8080, "verbose": true});
/// assert_eq!(config.value_str("host", "0.0.0.0"), "localhost");
/// assert_eq!(config.value_u16("port", 3000), 8080);
/// assert_eq!(config.value_bool("verbose", false), true);
/// assert_eq!(config.value_str("missing", "fallback"), "fallback");
/// ```
pub trait JsonValueExt {
    fn value_str(&self, key: &str, default: &str) -> String;
    fn value_f64(&self, key: &str, default: f64) -> f64;
    fn value_i64(&self, key: &str, default: i64) -> i64;
    fn value_u64(&self, key: &str, default: u64) -> u64;
    fn value_bool(&self, key: &str, default: bool) -> bool;
    fn value_u16(&self, key: &str, default: u16) -> u16;
    fn value_u32(&self, key: &str, default: u32) -> u32;
}

impl JsonValueExt for serde_json::Value {
    fn value_str(&self, key: &str, default: &str) -> String {
        self.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    }

    fn value_f64(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
    }

    fn value_i64(&self, key: &str, default: i64) -> i64 {
        self.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
    }

    fn value_u64(&self, key: &str, default: u64) -> u64 {
        self.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
    }

    fn value_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    fn value_u16(&self, key: &str, default: u16) -> u16 {
        self.get(key)
            .and_then(|v| v.as_u64())
            .map(|n| n as u16)
            .unwrap_or(default)
    }

    fn value_u32(&self, key: &str, default: u32) -> u32 {
        self.get(key)
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(default)
    }
}

// ---------------------------------------------------------------------------
// Type aliases matching C++ LP::* types
// ---------------------------------------------------------------------------

pub type Vec2d = Vector2<f64>;
pub type Vec3d = Vector3<f64>;
pub type Quatd = UnitQuaternion<f64>;
pub type Mat3d = Matrix3<f64>;
pub type Trafo3d = Isometry3<f64>;
pub type AngleAxisd = Rotation3<f64>;
pub type TimePoint = SystemTime;

// Re-export Duration so callers can use fusion_types::Duration.
pub use std::time::Duration as FusionDuration;

fn default_time_point() -> TimePoint {
    SystemTime::UNIX_EPOCH
}

fn default_duration() -> Duration {
    Duration::from_micros(11_111) // 90 fps default
}

// ---------------------------------------------------------------------------
// Helper: serde for SystemTime as nanoseconds since UNIX epoch (C++ compat)
// ---------------------------------------------------------------------------

mod serde_system_time {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(tp: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let nanos = tp
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as i64;
        nanos.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let nanos = i64::deserialize(d)?;
        let nanos_u = if nanos < 0 { 0u64 } else { nanos as u64 };
        Ok(UNIX_EPOCH + Duration::from_nanos(nanos_u))
    }
}

// ---------------------------------------------------------------------------
// Helper: serde for Vec3d as {"x": .., "y": .., "z": ..} (C++ compat)
// ---------------------------------------------------------------------------

mod serde_vec3d {
    use super::Vec3d;
    use serde::de::{self, SeqAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(v: &Vec3d, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(3))?;
        map.serialize_entry("x", &v.x)?;
        map.serialize_entry("y", &v.y)?;
        map.serialize_entry("z", &v.z)?;
        map.end()
    }

    struct Vec3dVisitor;
    impl<'de> Visitor<'de> for Vec3dVisitor {
        type Value = Vec3d;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "an object {{x,y,z}} or array [x,y,z]")
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Vec3d, A::Error> {
            let mut x = None;
            let mut y = None;
            let mut z = None;
            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "x" => x = Some(map.next_value()?),
                    "y" => y = Some(map.next_value()?),
                    "z" => z = Some(map.next_value()?),
                    _ => { let _ = map.next_value::<serde::de::IgnoredAny>()?; }
                }
            }
            Ok(Vec3d::new(
                x.ok_or_else(|| de::Error::missing_field("x"))?,
                y.ok_or_else(|| de::Error::missing_field("y"))?,
                z.ok_or_else(|| de::Error::missing_field("z"))?,
            ))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec3d, A::Error> {
            let x = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(0, &self))?;
            let y = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(1, &self))?;
            let z = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(2, &self))?;
            Ok(Vec3d::new(x, y, z))
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec3d, D::Error> {
        d.deserialize_any(Vec3dVisitor)
    }
}

// ---------------------------------------------------------------------------
// Helper: serde for Vec2d as {"longitude": .., "latitude": ..} (C++ compat)
// The C++ code always uses longitude/latitude for Vec2d serialization.
// ---------------------------------------------------------------------------

mod serde_vec2d_lonlat {
    use super::Vec2d;
    use serde::de::{self, SeqAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(v: &Vec2d, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("longitude", &v.x)?;
        map.serialize_entry("latitude", &v.y)?;
        map.end()
    }

    struct Vec2dVisitor;
    impl<'de> Visitor<'de> for Vec2dVisitor {
        type Value = Vec2d;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "an object {{longitude,latitude}} or array [lon,lat]")
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Vec2d, A::Error> {
            let mut x = None;
            let mut y = None;
            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "longitude" | "x" => x = Some(map.next_value()?),
                    "latitude" | "y" => y = Some(map.next_value()?),
                    _ => { let _ = map.next_value::<serde::de::IgnoredAny>()?; }
                }
            }
            Ok(Vec2d::new(
                x.ok_or_else(|| de::Error::missing_field("longitude"))?,
                y.ok_or_else(|| de::Error::missing_field("latitude"))?,
            ))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec2d, A::Error> {
            let x = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(0, &self))?;
            let y = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(1, &self))?;
            Ok(Vec2d::new(x, y))
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec2d, D::Error> {
        d.deserialize_any(Vec2dVisitor)
    }
}

// ---------------------------------------------------------------------------
// Helper: serde for Quatd as {"w": .., "x": .., "y": .., "z": ..} (C++ compat)
// ---------------------------------------------------------------------------

mod serde_quatd {
    use super::Quatd;
    use serde::de::{self, SeqAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(q: &Quatd, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(4))?;
        map.serialize_entry("w", &q.w)?;
        map.serialize_entry("x", &q.i)?;
        map.serialize_entry("y", &q.j)?;
        map.serialize_entry("z", &q.k)?;
        map.end()
    }

    struct QuatdVisitor;
    impl<'de> Visitor<'de> for QuatdVisitor {
        type Value = Quatd;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "an object {{w,x,y,z}} or array [w,x,y,z]")
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Quatd, A::Error> {
            let mut w = None;
            let mut x = None;
            let mut y = None;
            let mut z = None;
            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "w" => w = Some(map.next_value()?),
                    "x" => x = Some(map.next_value()?),
                    "y" => y = Some(map.next_value()?),
                    "z" => z = Some(map.next_value()?),
                    _ => { let _ = map.next_value::<serde::de::IgnoredAny>()?; }
                }
            }
            let q = nalgebra::Quaternion::new(
                w.ok_or_else(|| de::Error::missing_field("w"))?,
                x.ok_or_else(|| de::Error::missing_field("x"))?,
                y.ok_or_else(|| de::Error::missing_field("y"))?,
                z.ok_or_else(|| de::Error::missing_field("z"))?,
            );
            Ok(Quatd::from_quaternion(q))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Quatd, A::Error> {
            let w: f64 = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(0, &self))?;
            let x: f64 = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(1, &self))?;
            let y: f64 = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(2, &self))?;
            let z: f64 = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(3, &self))?;
            let q = nalgebra::Quaternion::new(w, x, y, z);
            Ok(Quatd::from_quaternion(q))
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Quatd, D::Error> {
        d.deserialize_any(QuatdVisitor)
    }
}

// ---------------------------------------------------------------------------
// 1. ImuData
// C++ serializes: timestamp, senderId, gyroscope, accelerometer, quaternion, euler
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImuData {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(skip)]
    pub latency: f64,
    #[serde(with = "serde_vec3d")]
    pub gyroscope: Vec3d,
    #[serde(with = "serde_vec3d")]
    pub accelerometer: Vec3d,
    #[serde(with = "serde_quatd")]
    pub quaternion: Quatd,
    #[serde(with = "serde_vec3d")]
    pub euler: Vec3d,
    #[serde(skip)]
    pub period: f64,
    #[serde(skip)]
    pub internal_frame_count: i32,
    #[serde(skip)]
    pub linear_velocity: Vec3d,
}

impl Default for ImuData {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            latency: 0.0,
            gyroscope: Vec3d::zeros(),
            accelerometer: Vec3d::zeros(),
            quaternion: Quatd::identity(),
            euler: Vec3d::zeros(),
            period: 0.0,
            internal_frame_count: 0,
            linear_velocity: Vec3d::zeros(),
        }
    }
}

// ---------------------------------------------------------------------------
// 2. GnssData
// C++ serializes: timestamp, senderId, latitude, longitude, height, nSat, quality,
//                 hdop, tmg, orientation, altitude, undulation, heading, diffAge
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GnssData {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(skip)]
    pub latency: f64,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(skip)]
    pub horizontal_accuracy: f64,
    #[serde(skip)]
    pub vertical_accuracy: f64,
    pub altitude: f64,
    pub undulation: f64,
    pub height: f64,
    pub quality: i32,
    #[serde(rename = "nSat")]
    pub n_sat: i32,
    pub hdop: f64,
    pub tmg: f64,
    pub heading: f64,
    #[serde(skip)]
    pub period: f64,
    #[serde(skip)]
    pub internal_frame_count: i32,
    #[serde(with = "serde_quatd")]
    pub orientation: Quatd,
    #[serde(rename = "diffAge", default)]
    pub diff_age: f64,
}

impl Default for GnssData {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            latency: 0.0,
            latitude: 0.0,
            longitude: 0.0,
            horizontal_accuracy: 0.0,
            vertical_accuracy: 0.0,
            altitude: 0.0,
            undulation: 0.0,
            height: 0.0,
            quality: 0,
            n_sat: 0,
            hdop: 0.0,
            tmg: 0.0,
            heading: 0.0,
            period: 0.0,
            internal_frame_count: 0,
            orientation: Quatd::identity(),
            diff_age: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// 3. OpticalData
// C++ serializes: timestamp, senderId, position, orientation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpticalData {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(skip, default = "default_time_point")]
    pub last_data_time: TimePoint,
    #[serde(skip)]
    pub latency: f64,
    #[serde(with = "serde_vec3d")]
    pub position: Vec3d,
    #[serde(with = "serde_quatd")]
    pub orientation: Quatd,
    #[serde(skip)]
    pub angular_velocity: Vec3d,
    #[serde(skip)]
    pub quality: f64,
    #[serde(skip)]
    pub frame_rate: f64,
    #[serde(skip)]
    pub frame_number: i32,
    #[serde(skip, default = "default_duration")]
    pub interval: Duration,
}

impl Default for OpticalData {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            last_data_time: SystemTime::UNIX_EPOCH,
            latency: 0.0,
            position: Vec3d::zeros(),
            orientation: Quatd::identity(),
            angular_velocity: Vec3d::zeros(),
            quality: 0.0,
            frame_rate: 0.0,
            frame_number: 0,
            interval: Duration::from_micros(11_111), // 90 fps default
        }
    }
}

// ---------------------------------------------------------------------------
// 4. FusedPose
// C++ serializes: timestamp, transmissionTime, senderId, lastDataTime,
//                 position, orientation, velocity, acceleration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FusedPose {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(rename = "transmissionTime", with = "serde_system_time")]
    pub transmission_time: TimePoint,
    #[serde(rename = "lastDataTime", with = "serde_system_time")]
    pub last_data_time: TimePoint,
    #[serde(with = "serde_vec3d")]
    pub position: Vec3d,
    #[serde(with = "serde_quatd")]
    pub orientation: Quatd,
    #[serde(skip)]
    pub angular_velocity: Vec3d,
    #[serde(with = "serde_vec3d")]
    pub velocity: Vec3d,
    #[serde(with = "serde_vec3d")]
    pub acceleration: Vec3d,
    #[serde(skip)]
    pub frame_number: i64,
    #[serde(skip)]
    pub latency: f64,
}

impl Default for FusedPose {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            transmission_time: SystemTime::UNIX_EPOCH,
            last_data_time: SystemTime::UNIX_EPOCH,
            position: Vec3d::zeros(),
            orientation: Quatd::identity(),
            angular_velocity: Vec3d::zeros(),
            velocity: Vec3d::zeros(),
            acceleration: Vec3d::zeros(),
            frame_number: 0,
            latency: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// 5. FusedVehiclePose
// C++ serializes: timestamp, timecode, position, globalPosition, yaw, utmZone, acceleration
// C++ Vec2d always serializes as {longitude, latitude}
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FusedVehiclePose {
    #[serde(rename = "senderId", skip)]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(with = "serde_system_time")]
    pub timecode: TimePoint,
    #[serde(with = "serde_vec2d_lonlat")]
    pub position: Vec2d,
    pub yaw: f64,
    #[serde(rename = "utmZone")]
    pub utm_zone: String,
    #[serde(rename = "globalPosition", with = "serde_vec2d_lonlat")]
    pub global_position: Vec2d,
    #[serde(with = "serde_vec3d")]
    pub acceleration: Vec3d,
}

impl Default for FusedVehiclePose {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            timecode: SystemTime::UNIX_EPOCH,
            position: Vec2d::zeros(),
            yaw: 0.0,
            utm_zone: String::new(),
            global_position: Vec2d::zeros(),
            acceleration: Vec3d::zeros(),
        }
    }
}

impl From<FusedVehiclePoseV2> for FusedVehiclePose {
    fn from(p: FusedVehiclePoseV2) -> Self {
        Self {
            sender_id: p.sender_id,
            timestamp: p.timestamp,
            timecode: p.timestamp,
            position: p.position,
            yaw: p.yaw,
            utm_zone: p.utm_zone,
            global_position: p.global_position,
            acceleration: Vec3d::new(p.acceleration.x, p.acceleration.y, 0.0),
        }
    }
}

// ---------------------------------------------------------------------------
// 6. FusedVehiclePoseV2
// C++ serializes: timestamp, transmissionTime, senderId, position, globalPosition,
//                 yaw, utmZone, velocity, acceleration, angularVelocity, internalFrameCount
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FusedVehiclePoseV2 {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(rename = "transmissionTime", with = "serde_system_time")]
    pub transmission_time: TimePoint,
    #[serde(skip, default = "default_time_point")]
    pub last_data_time: TimePoint,
    #[serde(with = "serde_vec2d_lonlat")]
    pub position: Vec2d,
    pub yaw: f64,
    #[serde(rename = "utmZone")]
    pub utm_zone: String,
    #[serde(rename = "globalPosition", with = "serde_vec2d_lonlat")]
    pub global_position: Vec2d,
    #[serde(with = "serde_vec2d_lonlat")]
    pub acceleration: Vec2d,
    #[serde(with = "serde_vec2d_lonlat")]
    pub velocity: Vec2d,
    #[serde(rename = "angularVelocity")]
    pub angular_velocity: f64,
    #[serde(rename = "internalFrameCount")]
    pub internal_frame_count: i64,
}

impl Default for FusedVehiclePoseV2 {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            transmission_time: SystemTime::UNIX_EPOCH,
            last_data_time: SystemTime::UNIX_EPOCH,
            position: Vec2d::zeros(),
            yaw: 0.0,
            utm_zone: String::new(),
            global_position: Vec2d::zeros(),
            acceleration: Vec2d::zeros(),
            velocity: Vec2d::zeros(),
            angular_velocity: 0.0,
            internal_frame_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// 7. GpsPoint
// C++ serializes: timestamp, senderId, longitude, latitude, height
// (but the to_json only outputs longitude, latitude, height -- the from_json
//  reads those too. The NLOHMANN macro includes timestamp and senderId.)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpsPoint {
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(rename = "senderId")]
    pub sender_id: String,
    pub longitude: f64,
    pub latitude: f64,
    pub height: f64,
}

impl Default for GpsPoint {
    fn default() -> Self {
        Self {
            timestamp: SystemTime::UNIX_EPOCH,
            sender_id: String::new(),
            longitude: 0.0,
            latitude: 0.0,
            height: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// 8. GlobalFusedPose
// C++ serializes: timestamp, transmissionTime, senderId, position, orientation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GlobalFusedPose {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(rename = "transmissionTime", with = "serde_system_time")]
    pub transmission_time: TimePoint,
    #[serde(skip, default = "default_time_point")]
    pub last_data_time: TimePoint,
    pub position: GpsPoint,
    #[serde(with = "serde_quatd")]
    pub orientation: Quatd,
    #[serde(skip)]
    pub latency_in_ms: i32,
}

impl Default for GlobalFusedPose {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            transmission_time: SystemTime::UNIX_EPOCH,
            last_data_time: SystemTime::UNIX_EPOCH,
            position: GpsPoint::default(),
            orientation: Quatd::identity(),
            latency_in_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// 9. RTCMData
// C++ serializes: timestamp, senderId, length, chunk
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RTCMData {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    pub chunk: Vec<u8>,
    pub length: i32,
}

impl Default for RTCMData {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            chunk: Vec::new(),
            length: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// 10. CANData
// C++ serializes: timestamp, senderId, isExtended, id, data, length
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CANData {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(rename = "isExtended")]
    pub is_extended: bool,
    pub id: u32,
    pub length: i32,
    pub data: Vec<u8>,
}

impl Default for CANData {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            is_extended: false,
            id: 0,
            length: 0,
            data: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 11. VehicleState
// C++ serializes: timestamp, senderId, wheelBase, trackWidth, steeringAngleL,
//                 steeringAngleR, wheelFR, wheelFL, wheelRR, wheelRL
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VehicleState {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(rename = "wheelBase")]
    pub wheel_base: f64,
    #[serde(rename = "trackWidth")]
    pub track_width: f64,
    #[serde(rename = "steeringAngleL")]
    pub steering_angle_l: f64,
    #[serde(rename = "steeringAngleR")]
    pub steering_angle_r: f64,
    #[serde(skip)]
    pub steering_speed: f64,
    #[serde(rename = "wheelFR")]
    pub wheel_fr: f64,
    #[serde(rename = "wheelFL")]
    pub wheel_fl: f64,
    #[serde(rename = "wheelRR")]
    pub wheel_rr: f64,
    #[serde(rename = "wheelRL")]
    pub wheel_rl: f64,
    #[serde(skip)]
    pub timestamp_ns: f64,
}

impl Default for VehicleState {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            wheel_base: 1.0,
            track_width: 1.0,
            steering_angle_l: 0.0,
            steering_angle_r: 0.0,
            steering_speed: 0.0,
            wheel_fr: 0.0,
            wheel_fl: 0.0,
            wheel_rr: 0.0,
            wheel_rl: 0.0,
            timestamp_ns: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// 12. VehicleSpeed
// C++ serializes: timestamp, senderId, linear, angular, validAngular
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VehicleSpeed {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    pub linear: f64,
    pub angular: f64,
    #[serde(rename = "validAngular")]
    pub valid_angular: bool,
}

impl Default for VehicleSpeed {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            linear: 0.0,
            angular: 0.0,
            valid_angular: false,
        }
    }
}

// ---------------------------------------------------------------------------
// 13. VelocityMeterData
// C++ serializes: senderId, timestamp, counter, velocity, distance, material,
//                 dopplerLevel, outputStatus
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VelocityMeterData {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    pub counter: i32,
    pub velocity: f64,
    pub distance: f64,
    pub material: f64,
    #[serde(rename = "dopplerLevel")]
    pub doppler_level: f64,
    #[serde(rename = "outputStatus")]
    pub output_status: i32,
}

impl Default for VelocityMeterData {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            counter: 0,
            velocity: 0.0,
            distance: 0.0,
            material: 0.0,
            doppler_level: 0.0,
            output_status: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// 14. FusionStateInt
// C++ serializes: timestamp, senderId, position, velocity, gravity,
//                 imuOrientation, omegaBias, accelBias, imuPosition
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FusionStateInt {
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(with = "serde_system_time")]
    pub timestamp: TimePoint,
    #[serde(with = "serde_vec3d")]
    pub position: Vec3d,
    #[serde(with = "serde_vec3d")]
    pub velocity: Vec3d,
    pub gravity: f64,
    #[serde(rename = "imuOrientation", with = "serde_quatd")]
    pub imu_orientation: Quatd,
    #[serde(rename = "omegaBias", with = "serde_vec3d")]
    pub omega_bias: Vec3d,
    #[serde(rename = "accelBias", with = "serde_vec3d")]
    pub accel_bias: Vec3d,
    #[serde(rename = "imuPosition", with = "serde_vec3d")]
    pub imu_position: Vec3d,
}

impl Default for FusionStateInt {
    fn default() -> Self {
        Self {
            sender_id: String::new(),
            timestamp: SystemTime::UNIX_EPOCH,
            position: Vec3d::zeros(),
            velocity: Vec3d::zeros(),
            gravity: 0.0,
            imu_orientation: Quatd::identity(),
            omega_bias: Vec3d::zeros(),
            accel_bias: Vec3d::zeros(),
            imu_position: Vec3d::zeros(),
        }
    }
}

// ---------------------------------------------------------------------------
// 15. Timestamp
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Timestamp {
    #[serde(with = "serde_system_time")]
    pub now: TimePoint,
}

impl Default for Timestamp {
    fn default() -> Self {
        Self {
            now: SystemTime::UNIX_EPOCH,
        }
    }
}

impl Timestamp {
    pub fn current() -> Self {
        Self {
            now: SystemTime::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// 15b. Extension message codec registry
// ---------------------------------------------------------------------------

pub struct ExtensionCodec {
    pub type_name: &'static str,
    pub json_encode: fn(&dyn Any) -> Option<serde_json::Value>,
    pub json_decode: fn(&serde_json::Value) -> Option<Box<dyn Any + Send + Sync>>,
    pub proto_encode: fn(&dyn Any) -> Vec<u8>,
    pub proto_decode: fn(&[u8]) -> Option<Box<dyn Any + Send + Sync>>,
}

static EXT_CODEC_REGISTRY: OnceLock<Mutex<Vec<ExtensionCodec>>> = OnceLock::new();

fn ext_codec_registry() -> &'static Mutex<Vec<ExtensionCodec>> {
    EXT_CODEC_REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn register_extension_codec(codec: ExtensionCodec) {
    ext_codec_registry().lock().unwrap().push(codec);
}

pub fn encode_extension_json(type_name: &str, payload: &dyn Any) -> Option<serde_json::Value> {
    let reg = ext_codec_registry().lock().unwrap();
    reg.iter()
        .find(|c| c.type_name == type_name)
        .and_then(|c| (c.json_encode)(payload))
}

pub fn decode_extension_json(
    type_name: &str,
    value: &serde_json::Value,
) -> Option<Box<dyn Any + Send + Sync>> {
    let reg = ext_codec_registry().lock().unwrap();
    reg.iter()
        .find(|c| c.type_name == type_name)
        .and_then(|c| (c.json_decode)(value))
}

pub fn encode_extension_proto(type_name: &str, payload: &dyn Any) -> Option<Vec<u8>> {
    let reg = ext_codec_registry().lock().unwrap();
    reg.iter()
        .find(|c| c.type_name == type_name)
        .map(|c| (c.proto_encode)(payload))
}

pub fn decode_extension_proto(
    type_name: &str,
    bytes: &[u8],
) -> Option<Box<dyn Any + Send + Sync>> {
    let reg = ext_codec_registry().lock().unwrap();
    reg.iter()
        .find(|c| c.type_name == type_name)
        .and_then(|c| (c.proto_decode)(bytes))
}

pub fn extension_codec_type_names() -> Vec<&'static str> {
    let reg = ext_codec_registry().lock().unwrap();
    reg.iter().map(|c| c.type_name).collect()
}

// ---------------------------------------------------------------------------
// 15c. ExtensionEnvelope
// ---------------------------------------------------------------------------

pub struct ExtensionEnvelope {
    pub type_name: String,
    pub sender_id: String,
    pub timestamp: SystemTime,
    payload: Arc<dyn Any + Send + Sync>,
}

impl ExtensionEnvelope {
    pub fn new<T: Send + Sync + 'static>(
        type_name: impl Into<String>,
        sender_id: impl Into<String>,
        timestamp: SystemTime,
        data: T,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            sender_id: sender_id.into(),
            timestamp,
            payload: Arc::new(data),
        }
    }

    pub fn from_parts(
        type_name: impl Into<String>,
        sender_id: impl Into<String>,
        timestamp: SystemTime,
        payload: Box<dyn Any + Send + Sync>,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            sender_id: sender_id.into(),
            timestamp,
            payload: Arc::from(payload),
        }
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.payload.downcast_ref::<T>()
    }

    pub fn payload_any(&self) -> &dyn Any {
        &*self.payload
    }
}

impl Clone for ExtensionEnvelope {
    fn clone(&self) -> Self {
        Self {
            type_name: self.type_name.clone(),
            sender_id: self.sender_id.clone(),
            timestamp: self.timestamp,
            payload: Arc::clone(&self.payload),
        }
    }
}

impl fmt::Debug for ExtensionEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionEnvelope")
            .field("type_name", &self.type_name)
            .field("sender_id", &self.sender_id)
            .field("timestamp", &self.timestamp)
            .finish_non_exhaustive()
    }
}

impl Serialize for ExtensionEnvelope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let payload_json = encode_extension_json(&self.type_name, &*self.payload);
        let mut state = serializer.serialize_struct("ExtensionEnvelope", 4)?;
        state.serialize_field("typeName", &self.type_name)?;
        state.serialize_field("senderId", &self.sender_id)?;
        state.serialize_field(
            "timestamp",
            &self
                .timestamp
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_nanos(),
        )?;
        state.serialize_field("payload", &payload_json)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for ExtensionEnvelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            type_name: String,
            sender_id: String,
            timestamp: u128,
            payload: Option<serde_json::Value>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let timestamp =
            SystemTime::UNIX_EPOCH + Duration::from_nanos(raw.timestamp as u64);
        let payload: Arc<dyn Any + Send + Sync> = raw
            .payload
            .as_ref()
            .and_then(|v| decode_extension_json(&raw.type_name, v))
            .map(|b| Arc::from(b) as Arc<dyn Any + Send + Sync>)
            .unwrap_or_else(|| Arc::new(()) as Arc<dyn Any + Send + Sync>);
        Ok(Self {
            type_name: raw.type_name,
            sender_id: raw.sender_id,
            timestamp,
            payload,
        })
    }
}

// ---------------------------------------------------------------------------
// 16. StreamableData
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamableData {
    Imu(ImuData),
    Gnss(GnssData),
    Optical(OpticalData),
    FusedPose(FusedPose),
    FusedVehiclePose(FusedVehiclePose),
    FusedVehiclePoseV2(FusedVehiclePoseV2),
    GlobalFusedPose(GlobalFusedPose),
    FusionStateInt(FusionStateInt),
    Rtcm(RTCMData),
    Can(CANData),
    VehicleState(VehicleState),
    VehicleSpeed(VehicleSpeed),
    VelocityMeter(VelocityMeterData),
    Timestamp(Timestamp),
    Extension(ExtensionEnvelope),
}

impl StreamableData {
    /// Return the variant name as a string (for data type filtering).
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::Imu(_) => "Imu",
            Self::Gnss(_) => "Gnss",
            Self::Optical(_) => "Optical",
            Self::FusedPose(_) => "FusedPose",
            Self::FusedVehiclePose(_) => "FusedVehiclePose",
            Self::FusedVehiclePoseV2(_) => "FusedVehiclePoseV2",
            Self::GlobalFusedPose(_) => "GlobalFusedPose",
            Self::FusionStateInt(_) => "FusionStateInt",
            Self::Rtcm(_) => "Rtcm",
            Self::Can(_) => "Can",
            Self::VehicleState(_) => "VehicleState",
            Self::VehicleSpeed(_) => "VehicleSpeed",
            Self::VelocityMeter(_) => "VelocityMeter",
            Self::Timestamp(_) => "Timestamp",
            Self::Extension(_) => "Extension",
        }
    }

    /// Get the extension type name (only for Extension variants).
    pub fn extension_type_name(&self) -> Option<&str> {
        match self {
            Self::Extension(e) => Some(&e.type_name),
            _ => None,
        }
    }

    /// Get the sender_id for data variants that have one.
    pub fn sender_id(&self) -> Option<&str> {
        match self {
            Self::Imu(d) => Some(&d.sender_id),
            Self::Gnss(d) => Some(&d.sender_id),
            Self::Optical(d) => Some(&d.sender_id),
            Self::FusedPose(d) => Some(&d.sender_id),
            Self::FusedVehiclePose(d) => Some(&d.sender_id),
            Self::FusedVehiclePoseV2(d) => Some(&d.sender_id),
            Self::GlobalFusedPose(d) => Some(&d.sender_id),
            Self::FusionStateInt(d) => Some(&d.sender_id),
            Self::Rtcm(d) => Some(&d.sender_id),
            Self::Can(d) => Some(&d.sender_id),
            Self::VehicleState(d) => Some(&d.sender_id),
            Self::VehicleSpeed(d) => Some(&d.sender_id),
            Self::VelocityMeter(d) => Some(&d.sender_id),
            Self::Timestamp(_) => None,
            Self::Extension(e) => Some(&e.sender_id),
        }
    }

    /// Set the sender_id on data variants that support it.
    pub fn set_sender_id(&mut self, id: &str) {
        match self {
            Self::Imu(d) => d.sender_id = id.to_owned(),
            Self::Gnss(d) => d.sender_id = id.to_owned(),
            Self::Optical(d) => d.sender_id = id.to_owned(),
            Self::FusedPose(d) => d.sender_id = id.to_owned(),
            Self::FusedVehiclePose(d) => d.sender_id = id.to_owned(),
            Self::FusedVehiclePoseV2(d) => d.sender_id = id.to_owned(),
            Self::GlobalFusedPose(d) => d.sender_id = id.to_owned(),
            Self::FusionStateInt(d) => d.sender_id = id.to_owned(),
            Self::Rtcm(d) => d.sender_id = id.to_owned(),
            Self::Can(d) => d.sender_id = id.to_owned(),
            Self::VehicleState(d) => d.sender_id = id.to_owned(),
            Self::VehicleSpeed(d) => d.sender_id = id.to_owned(),
            Self::VelocityMeter(d) => d.sender_id = id.to_owned(),
            Self::Timestamp(_) => {}
            Self::Extension(e) => e.sender_id = id.to_owned(),
        }
    }

    /// Get the timestamp for data variants that have one.
    pub fn timestamp(&self) -> Option<TimePoint> {
        match self {
            Self::Imu(d) => Some(d.timestamp),
            Self::Gnss(d) => Some(d.timestamp),
            Self::Optical(d) => Some(d.timestamp),
            Self::FusedPose(d) => Some(d.timestamp),
            Self::FusedVehiclePose(d) => Some(d.timestamp),
            Self::FusedVehiclePoseV2(d) => Some(d.timestamp),
            Self::GlobalFusedPose(d) => Some(d.timestamp),
            Self::FusionStateInt(d) => Some(d.timestamp),
            Self::Rtcm(d) => Some(d.timestamp),
            Self::Can(d) => Some(d.timestamp),
            Self::VehicleState(d) => Some(d.timestamp),
            Self::VehicleSpeed(d) => Some(d.timestamp),
            Self::VelocityMeter(d) => Some(d.timestamp),
            Self::Timestamp(t) => Some(t.now),
            Self::Extension(e) => Some(e.timestamp),
        }
    }

    /// Set the timestamp on data variants that support it.
    pub fn set_timestamp(&mut self, ts: TimePoint) {
        match self {
            Self::Imu(d) => d.timestamp = ts,
            Self::Gnss(d) => d.timestamp = ts,
            Self::Optical(d) => d.timestamp = ts,
            Self::FusedPose(d) => d.timestamp = ts,
            Self::FusedVehiclePose(d) => d.timestamp = ts,
            Self::FusedVehiclePoseV2(d) => d.timestamp = ts,
            Self::GlobalFusedPose(d) => d.timestamp = ts,
            Self::FusionStateInt(d) => d.timestamp = ts,
            Self::Rtcm(d) => d.timestamp = ts,
            Self::Can(d) => d.timestamp = ts,
            Self::VehicleState(d) => d.timestamp = ts,
            Self::VehicleSpeed(d) => d.timestamp = ts,
            Self::VelocityMeter(d) => d.timestamp = ts,
            Self::Timestamp(t) => t.now = ts,
            Self::Extension(e) => e.timestamp = ts,
        }
    }

    /// Returns true if this is a Timestamp variant.
    pub fn is_timestamp(&self) -> bool {
        matches!(self, Self::Timestamp(_))
    }
}

// ---------------------------------------------------------------------------
// 17. ApiRequest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiRequest {
    pub command: String,
    pub topic: String,
    pub data: serde_json::Value,
    pub id: String,
}

impl Default for ApiRequest {
    fn default() -> Self {
        Self {
            command: String::new(),
            topic: String::new(),
            data: serde_json::Value::Null,
            id: String::new(),
        }
    }
}

impl ApiRequest {
    pub fn new(
        command: impl Into<String>,
        topic: impl Into<String>,
        data: serde_json::Value,
        id: impl Into<String>,
    ) -> Self {
        Self {
            command: command.into(),
            topic: topic.into(),
            data,
            id: id.into(),
        }
    }

    pub fn from_json(json: &serde_json::Value) -> Option<Self> {
        Some(Self {
            command: json.get("command")?.as_str()?.to_owned(),
            topic: json.value_str("topic", ""),
            data: json.get("data").cloned().unwrap_or(serde_json::Value::Null),
            id: json.value_str("id", ""),
        })
    }

    pub fn get_param(&self, name: &str) -> Option<&serde_json::Value> {
        self.data.get(name)
    }

    pub fn has_param(&self, name: &str) -> bool {
        self.data.get(name).is_some()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imu_data_default() {
        let d = ImuData::default();
        assert_eq!(d.timestamp, SystemTime::UNIX_EPOCH);
        assert_eq!(d.gyroscope, Vec3d::zeros());
        assert_eq!(d.quaternion, Quatd::identity());
    }

    #[test]
    fn fused_vehicle_pose_from_v2() {
        let v2 = FusedVehiclePoseV2 {
            sender_id: "test".into(),
            position: Vec2d::new(1.0, 2.0),
            yaw: 0.5,
            utm_zone: "32U".into(),
            global_position: Vec2d::new(11.0, 48.0),
            acceleration: Vec2d::new(0.1, 0.2),
            ..Default::default()
        };
        let v1 = FusedVehiclePose::from(v2.clone());
        assert_eq!(v1.position, v2.position);
        assert_eq!(v1.yaw, v2.yaw);
        assert_eq!(v1.acceleration.z, 0.0);
        assert_eq!(v1.acceleration.x, v2.acceleration.x);
    }

    #[test]
    fn streamable_data_roundtrip() {
        let data = StreamableData::Imu(ImuData::default());
        let json = serde_json::to_string(&data).unwrap();
        let back: StreamableData = serde_json::from_str(&json).unwrap();
        match back {
            StreamableData::Imu(imu) => assert_eq!(imu.period, 0.0),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn api_request_from_json() {
        let json: serde_json::Value = serde_json::json!({
            "command": "start",
            "topic": "imu0",
            "data": { "rate": 100 },
            "id": "req-1"
        });
        let req = ApiRequest::from_json(&json).unwrap();
        assert_eq!(req.command, "start");
        assert_eq!(req.topic, "imu0");
        assert!(req.has_param("rate"));
        assert_eq!(req.get_param("rate").unwrap(), &serde_json::json!(100));
    }

    #[test]
    fn timestamp_current() {
        let ts = Timestamp::current();
        assert!(ts.now > SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn json_value_ext_all_types() {
        let config = serde_json::json!({
            "host": "localhost",
            "port": 8080,
            "rate": 100.5,
            "verbose": true,
            "count": -42,
            "big": 999999
        });
        assert_eq!(config.value_str("host", "0.0.0.0"), "localhost");
        assert_eq!(config.value_str("missing", "fallback"), "fallback");
        assert_eq!(config.value_u16("port", 3000), 8080);
        assert_eq!(config.value_u32("port", 3000), 8080);
        assert_eq!(config.value_u64("port", 3000), 8080);
        assert!((config.value_f64("rate", 0.0) - 100.5).abs() < 1e-9);
        assert_eq!(config.value_f64("missing", 1.0), 1.0);
        assert_eq!(config.value_bool("verbose", false), true);
        assert_eq!(config.value_bool("missing", false), false);
        assert_eq!(config.value_i64("count", 0), -42);
        assert_eq!(config.value_i64("missing", 99), 99);
    }

    #[test]
    fn fused_pose_json_matches_cpp_format() {
        use std::time::{Duration, UNIX_EPOCH};
        let ts = UNIX_EPOCH + Duration::from_nanos(1772137433109455200);
        let fp = FusedPose {
            sender_id: "/sinks/fusion/settings".into(),
            timestamp: ts,
            transmission_time: UNIX_EPOCH,
            last_data_time: UNIX_EPOCH,
            position: Vec3d::zeros(),
            orientation: Quatd::from_quaternion(nalgebra::Quaternion::new(
                0.009526392636495448, 0.009362874074581006, 0.7085831102307157, 0.705500928651525,
            )),
            angular_velocity: Vec3d::zeros(),
            velocity: Vec3d::zeros(),
            acceleration: Vec3d::new(-0.00010963330856322145, 0.0002469858365108813, 0.0001497355501149933),
            frame_number: 0,
            latency: 0.0,
        };
        let json_str = serde_json::to_string(&fp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(v.get("senderId").is_some());
        assert!(v.get("sender_id").is_none());
        assert!(v.get("transmissionTime").is_some());
        assert!(v.get("lastDataTime").is_some());
        assert_eq!(v["timestamp"].as_i64().unwrap(), 1772137433109455200i64);
        let pos = &v["position"];
        assert!(pos.get("x").is_some());
        assert!(pos.get("y").is_some());
        assert!(pos.get("z").is_some());
        let ori = &v["orientation"];
        assert!(ori.get("w").is_some());
        assert!(ori.get("x").is_some());
        // Skipped fields should not appear
        assert!(v.get("angular_velocity").is_none());
        assert!(v.get("angularVelocity").is_none());
        assert!(v.get("frame_number").is_none());
        assert!(v.get("frameNumber").is_none());
        assert!(v.get("latency").is_none());
    }

    #[test]
    fn fused_pose_decode_from_cpp_json() {
        let cpp_json = r#"{"timestamp":1772137433109455200,"transmissionTime":0,"senderId":"/sinks/fusion/settings","lastDataTime":0,"position":{"x":0.0,"y":0.0,"z":0.0},"orientation":{"w":0.009526392636495448,"x":0.009362874074581006,"y":0.7085831102307157,"z":0.705500928651525},"velocity":{"x":0.0,"y":0.0,"z":0.0},"acceleration":{"x":-0.00010963330856322145,"y":0.0002469858365108813,"z":0.0001497355501149933}}"#;
        let fp: FusedPose = serde_json::from_str(cpp_json).unwrap();
        assert_eq!(fp.sender_id, "/sinks/fusion/settings");
        assert!((fp.orientation.w - 0.009526392636495448).abs() < 1e-10);
        assert!((fp.acceleration.x - (-0.00010963330856322145)).abs() < 1e-15);
    }

    #[test]
    fn imu_data_json_camel_case() {
        let imu = ImuData {
            sender_id: "imu0".into(),
            gyroscope: Vec3d::new(1.0, 2.0, 3.0),
            accelerometer: Vec3d::new(0.1, 0.2, 9.81),
            ..Default::default()
        };
        let json_str = serde_json::to_string(&imu).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(v.get("senderId").is_some());
        assert_eq!(v["gyroscope"]["x"].as_f64().unwrap(), 1.0);
        assert_eq!(v["accelerometer"]["z"].as_f64().unwrap(), 9.81);
        // Skipped fields
        assert!(v.get("latency").is_none());
        assert!(v.get("period").is_none());
        assert!(v.get("linear_velocity").is_none());
        assert!(v.get("internal_frame_count").is_none());
    }

    #[test]
    fn vec2d_serializes_as_lonlat() {
        let vp = FusedVehiclePose {
            position: Vec2d::new(100.0, 200.0),
            global_position: Vec2d::new(11.5, 48.2),
            ..Default::default()
        };
        let json_str = serde_json::to_string(&vp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["position"]["longitude"].as_f64().unwrap(), 100.0);
        assert_eq!(v["position"]["latitude"].as_f64().unwrap(), 200.0);
        assert_eq!(v["globalPosition"]["longitude"].as_f64().unwrap(), 11.5);
        assert_eq!(v["globalPosition"]["latitude"].as_f64().unwrap(), 48.2);
    }

    #[test]
    fn timestamp_nanoseconds() {
        use std::time::{Duration, UNIX_EPOCH};
        let ts_nanos: i64 = 1772137433109455200;
        let tp = UNIX_EPOCH + Duration::from_nanos(ts_nanos as u64);
        let data = FusedPose {
            timestamp: tp,
            ..Default::default()
        };
        let json_str = serde_json::to_string(&data).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["timestamp"].as_i64().unwrap(), ts_nanos);
    }
}
