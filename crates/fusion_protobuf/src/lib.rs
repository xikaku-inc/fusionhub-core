use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fusion_types::{
    encode_extension_proto, decode_extension_proto, ExtensionEnvelope,
    CANData, FusedPose, FusedVehiclePose, FusedVehiclePoseV2, FusionStateInt, GlobalFusedPose,
    GnssData, GpsPoint, ImuData, OpticalData, RTCMData, StreamableData, Vec2d, Vec3d,
    VehicleSpeed, VehicleState, VelocityMeterData,
};
use nalgebra::UnitQuaternion;
use prost::Message;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/fusion.proto.rs"));
}

// ---------------------------------------------------------------------------
// Timestamp helpers — nanoseconds since UNIX epoch, matching C++
// ---------------------------------------------------------------------------

fn time_to_nanos(t: &SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as i64
}

fn nanos_to_time(ns: i64) -> SystemTime {
    if ns <= 0 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::from_nanos(ns as u64)
}

// ---------------------------------------------------------------------------
// Vector / quaternion helpers
// ---------------------------------------------------------------------------

fn vec3_to_proto(v: &Vec3d) -> proto::Vector {
    proto::Vector {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

fn vec2_to_proto(v: &Vec2d) -> proto::Vector2 {
    proto::Vector2 { x: v.x, y: v.y }
}

fn quat_to_proto(q: &UnitQuaternion<f64>) -> proto::Quaternion {
    proto::Quaternion {
        w: q.w,
        x: q.i,
        y: q.j,
        z: q.k,
    }
}

fn proto_to_vec3(v: &proto::Vector) -> Vec3d {
    Vec3d::new(v.x, v.y, v.z)
}

fn proto_to_vec3_opt(v: &Option<proto::Vector>) -> Vec3d {
    v.as_ref().map(proto_to_vec3).unwrap_or_else(Vec3d::zeros)
}

fn proto_to_vec2(v: &proto::Vector2) -> Vec2d {
    Vec2d::new(v.x, v.y)
}

fn proto_to_vec2_opt(v: &Option<proto::Vector2>) -> Vec2d {
    v.as_ref().map(proto_to_vec2).unwrap_or_else(Vec2d::zeros)
}

fn proto_to_quat(q: &proto::Quaternion) -> UnitQuaternion<f64> {
    let quat = nalgebra::Quaternion::new(q.w, q.x, q.y, q.z);
    if quat.norm() < 1e-15 {
        UnitQuaternion::identity()
    } else {
        UnitQuaternion::new_normalize(quat)
    }
}

fn proto_to_quat_opt(q: &Option<proto::Quaternion>) -> UnitQuaternion<f64> {
    q.as_ref()
        .map(proto_to_quat)
        .unwrap_or_else(UnitQuaternion::identity)
}

// ---------------------------------------------------------------------------
// Encoder — matches C++ ProtobufEncoder exactly
// ---------------------------------------------------------------------------

pub struct ProtobufEncoder {
    m_count: AtomicI32,
}

impl ProtobufEncoder {
    pub fn new() -> Self {
        Self {
            m_count: AtomicI32::new(0),
        }
    }

    fn next_seq(&self) -> i32 {
        self.m_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn encode(&self, data: &StreamableData) -> Vec<u8> {
        let mut sd = proto::StreamData::default();

        match data {
            StreamableData::Imu(i) => {
                sd.sequence_number = self.next_seq();
                sd.imu_data = Some(encode_imu(i));
            }
            StreamableData::Gnss(g) => {
                sd.sequence_number = self.next_seq();
                sd.gnss_data = Some(encode_gnss(g));
            }
            StreamableData::Optical(o) => {
                sd.sequence_number = self.next_seq();
                sd.optical_data = Some(encode_optical(o));
            }
            StreamableData::FusedPose(f) => {
                sd.sequence_number = self.next_seq();
                sd.fused_pose = Some(encode_fused_pose(f));
            }
            StreamableData::FusedVehiclePose(f) => {
                sd.sequence_number = self.next_seq();
                sd.fused_vehicle_pose = Some(encode_fused_vehicle_pose(f));
            }
            StreamableData::FusedVehiclePoseV2(f) => {
                sd.sequence_number = self.next_seq();
                sd.fused_vehicle_pose_v2 = Some(encode_fused_vehicle_pose_v2(f));
            }
            StreamableData::GlobalFusedPose(g) => {
                sd.sequence_number = self.next_seq();
                sd.global_fused_pose = Some(encode_global_fused_pose(g));
            }
            StreamableData::FusionStateInt(f) => {
                sd.sequence_number = self.next_seq();
                sd.fusion_state_int = Some(encode_fusion_state_int(f));
            }
            StreamableData::Rtcm(r) => {
                sd.sequence_number = self.next_seq();
                sd.rtcm_data = Some(encode_rtcm(r));
            }
            StreamableData::Can(c) => {
                sd.sequence_number = self.next_seq();
                sd.can_data = Some(encode_can(c));
            }
            StreamableData::VehicleState(v) => {
                sd.sequence_number = self.next_seq();
                sd.vehicle_state = Some(encode_vehicle_state(v));
            }
            StreamableData::VehicleSpeed(v) => {
                sd.sequence_number = self.next_seq();
                sd.vehicle_speed = Some(encode_vehicle_speed(v));
            }
            StreamableData::VelocityMeter(v) => {
                sd.sequence_number = self.next_seq();
                sd.velocity_meter_data = Some(encode_velocity_meter(v));
            }
            StreamableData::Timestamp(_) => {
                sd.sequence_number = self.next_seq();
            }
            StreamableData::Reset => {
                return Vec::new();
            }
            StreamableData::Extension(e) => {
                sd.sequence_number = self.next_seq();
                let payload = encode_extension_proto(&e.type_name, e.payload_any())
                    .unwrap_or_default();
                sd.extension_data = Some(proto::ExtensionData {
                    type_name: e.type_name.clone(),
                    payload,
                    timestamp: time_to_nanos(&e.timestamp),
                    sender_id: e.sender_id.clone(),
                });
            }
        }

        sd.encode_to_vec()
    }
}

impl Default for ProtobufEncoder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Per-type encode functions — field-for-field match with C++ toProtobuf()
// ---------------------------------------------------------------------------

fn encode_imu(i: &ImuData) -> proto::ImuData {
    let ts = time_to_nanos(&i.timestamp);
    proto::ImuData {
        timestamp: ts,
        sender_id: i.sender_id.clone(),
        latency: i.latency,
        gyroscope: Some(vec3_to_proto(&i.gyroscope)),
        accelerometer: Some(vec3_to_proto(&i.accelerometer)),
        period: i.period,
        frame_count: i.internal_frame_count,
        quaternion: Some(quat_to_proto(&i.quaternion)),
        euler: Some(vec3_to_proto(&i.euler)),
        linear_velocity: Some(vec3_to_proto(&i.linear_velocity)),
        // V1 legacy fields
        sensor_name: i.sender_id.clone(),
        timecode: ts,
        fake_timecode: false,
        start_tick: 0,
        recorded_time: 0,
        sensor_time: 0,
    }
}

fn encode_gnss(g: &GnssData) -> proto::GnssData {
    let ts = time_to_nanos(&g.timestamp);
    proto::GnssData {
        timestamp: ts,
        sender_id: g.sender_id.clone(),
        latency: g.latency,
        latitude: g.latitude,
        longitude: g.longitude,
        height: g.height,
        vertical_accuracy: g.vertical_accuracy,
        horizontal_accuracy: g.horizontal_accuracy,
        quality: g.quality,
        n_sat: g.n_sat,
        hdop: g.hdop,
        tmg: g.tmg,
        heading: g.heading,
        altitude: g.altitude,
        undulation: g.undulation,
        period: g.period,
        frame_count: g.internal_frame_count,
        diff_age: g.diff_age,
        orientation: Some(quat_to_proto(&g.orientation)),
        // V1 legacy fields
        sensor_name: g.sender_id.clone(),
        timecode: ts,
        fake_timecode: false,
        start_tick: 0,
        recorded_time: 0,
        sensor_time: 0,
    }
}

fn encode_optical(o: &OpticalData) -> proto::OpticalData {
    let ts = time_to_nanos(&o.timestamp);
    proto::OpticalData {
        timestamp: ts,
        sender_id: o.sender_id.clone(),
        latency: o.latency,
        position: Some(vec3_to_proto(&o.position)),
        orientation: Some(quat_to_proto(&o.orientation)),
        angular_velocity: Some(vec3_to_proto(&o.angular_velocity)),
        quality: o.quality,
        frame_rate: o.frame_rate,
        frame_number: o.frame_number,
        // V1 legacy fields
        object_name: o.sender_id.clone(),
        timecode: ts,
        fake_timecode: false,
        recorded_time: 0,
    }
}

fn encode_fused_pose(f: &FusedPose) -> proto::FusedPose {
    proto::FusedPose {
        timestamp: time_to_nanos(&f.timestamp),
        transmission_time: time_to_nanos(&f.transmission_time),
        latency: f.latency,
        sender_id: f.sender_id.clone(),
        position: Some(vec3_to_proto(&f.position)),
        orientation: Some(quat_to_proto(&f.orientation)),
        angular_velocity: Some(vec3_to_proto(&f.angular_velocity)),
        velocity: Some(vec3_to_proto(&f.velocity)),
        acceleration: Some(vec3_to_proto(&f.acceleration)),
        frame_number: f.frame_number,
        // V1 legacy fields
        object_name: f.sender_id.clone(),
        timecode: time_to_nanos(&f.timestamp),
    }
}

fn encode_rtcm(r: &RTCMData) -> proto::RtcmData {
    proto::RtcmData {
        timestamp: time_to_nanos(&r.timestamp),
        sender_id: r.sender_id.clone(),
        chunk: r.chunk.clone(),
        length: r.length,
    }
}

fn encode_can(c: &CANData) -> proto::CanData {
    let ts = time_to_nanos(&c.timestamp);
    proto::CanData {
        timestamp: ts,
        sender_id: c.sender_id.clone(),
        is_extended: c.is_extended,
        id: c.id,
        data: c.data.clone(),
        length: c.length,
        // V1 legacy fields
        timecode: ts,
        recorded_time: 0,
    }
}

fn encode_vehicle_state(v: &VehicleState) -> proto::VehicleState {
    let ts = time_to_nanos(&v.timestamp);
    proto::VehicleState {
        timestamp: ts,
        sender_id: v.sender_id.clone(),
        wheel_base: v.wheel_base,
        track_width: v.track_width,
        steering_angle_l: v.steering_angle_l,
        steering_angle_r: v.steering_angle_r,
        wheel_fr: v.wheel_fr,
        wheel_fl: v.wheel_fl,
        wheel_rr: v.wheel_rr,
        wheel_rl: v.wheel_rl,
        // V1 legacy fields
        timecode: ts,
        recorded_time: 0,
    }
}

fn encode_vehicle_speed(v: &VehicleSpeed) -> proto::VehicleSpeed {
    let ts = time_to_nanos(&v.timestamp);
    proto::VehicleSpeed {
        timestamp: ts,
        sender_id: v.sender_id.clone(),
        linear: v.linear,
        angular: v.angular,
        valid_angular: v.valid_angular,
        // V1 legacy fields
        timecode: ts,
        recorded_time: 0,
    }
}

fn encode_velocity_meter(v: &VelocityMeterData) -> proto::VelocityMeterData {
    proto::VelocityMeterData {
        timestamp: time_to_nanos(&v.timestamp),
        sender_id: v.sender_id.clone(),
        counter: v.counter,
        velocity: v.velocity,
        distance: v.distance,
        material: v.material,
        doppler_level: v.doppler_level,
        output_status: v.output_status,
    }
}

fn encode_fused_vehicle_pose(f: &FusedVehiclePose) -> proto::FusedVehiclePose {
    proto::FusedVehiclePose {
        timestamp: time_to_nanos(&f.timestamp),
        timecode: time_to_nanos(&f.timecode),
        position: Some(vec2_to_proto(&f.position)),
        yaw: f.yaw,
        utm_zone: f.utm_zone.clone(),
        global_position: Some(vec2_to_proto(&f.global_position)),
        acceleration: Some(vec3_to_proto(&f.acceleration)),
    }
}

fn encode_fused_vehicle_pose_v2(f: &FusedVehiclePoseV2) -> proto::FusedVehiclePoseV2 {
    proto::FusedVehiclePoseV2 {
        timestamp: time_to_nanos(&f.timestamp),
        transmission_time: time_to_nanos(&f.transmission_time),
        sender_id: f.sender_id.clone(),
        position: Some(vec2_to_proto(&f.position)),
        yaw: f.yaw,
        angular_velocity: f.angular_velocity,
        utm_zone: f.utm_zone.clone(),
        global_position: Some(vec2_to_proto(&f.global_position)),
        velocity: Some(vec2_to_proto(&f.velocity)),
        acceleration: Some(vec2_to_proto(&f.acceleration)),
        internal_frame_count: f.internal_frame_count,
        timecode: time_to_nanos(&f.timestamp),
    }
}

fn encode_global_fused_pose(g: &GlobalFusedPose) -> proto::GlobalFusedPose {
    proto::GlobalFusedPose {
        timestamp: time_to_nanos(&g.timestamp),
        transmission_time: time_to_nanos(&g.transmission_time),
        sender_id: g.sender_id.clone(),
        position: Some(proto::GpsPoint {
            longitude: g.position.longitude,
            latitude: g.position.latitude,
            height: g.position.height,
        }),
        orientation: Some(quat_to_proto(&g.orientation)),
        timecode: time_to_nanos(&g.timestamp),
    }
}

fn encode_fusion_state_int(f: &FusionStateInt) -> proto::FusionStateInt {
    let ts = time_to_nanos(&f.timestamp);
    proto::FusionStateInt {
        timestamp: ts,
        sender_id: f.sender_id.clone(),
        position: Some(vec3_to_proto(&f.position)),
        velocity: Some(vec3_to_proto(&f.velocity)),
        gravity: f.gravity,
        imu_orientation: Some(quat_to_proto(&f.imu_orientation)),
        omega_bias: Some(vec3_to_proto(&f.omega_bias)),
        accel_bias: Some(vec3_to_proto(&f.accel_bias)),
        imu_position: Some(vec3_to_proto(&f.imu_position)),
        // V1 legacy fields
        timecode: ts,
        recorded_time: 0,
    }
}

// ---------------------------------------------------------------------------
// Decoder — matches C++ ProtobufDecoder exactly
// ---------------------------------------------------------------------------

pub fn decode(bytes: &[u8]) -> Option<StreamableData> {
    let sd = proto::StreamData::decode(bytes).ok()?;

    if let Some(ref d) = sd.imu_data {
        return Some(StreamableData::Imu(decode_imu(d)));
    }
    if let Some(ref d) = sd.optical_data {
        return Some(StreamableData::Optical(decode_optical(d)));
    }
    if let Some(ref d) = sd.fused_pose {
        return Some(StreamableData::FusedPose(decode_fused_pose(d)));
    }
    if let Some(ref d) = sd.gnss_data {
        return Some(StreamableData::Gnss(decode_gnss(d)));
    }
    if let Some(ref d) = sd.rtcm_data {
        return Some(StreamableData::Rtcm(decode_rtcm(d)));
    }
    if let Some(ref d) = sd.can_data {
        return Some(StreamableData::Can(decode_can(d)));
    }
    if let Some(ref d) = sd.vehicle_state {
        return Some(StreamableData::VehicleState(decode_vehicle_state(d)));
    }
    if let Some(ref d) = sd.fused_vehicle_pose {
        return Some(StreamableData::FusedVehiclePose(decode_fused_vehicle_pose(
            d,
        )));
    }
    if let Some(ref d) = sd.fused_vehicle_pose_v2 {
        return Some(StreamableData::FusedVehiclePoseV2(
            decode_fused_vehicle_pose_v2(d),
        ));
    }
    if let Some(ref d) = sd.global_fused_pose {
        return Some(StreamableData::GlobalFusedPose(decode_global_fused_pose(d)));
    }
    if let Some(ref d) = sd.fusion_state_int {
        return Some(StreamableData::FusionStateInt(decode_fusion_state_int(d)));
    }
    if let Some(ref d) = sd.vehicle_speed {
        return Some(StreamableData::VehicleSpeed(decode_vehicle_speed(d)));
    }
    if let Some(ref d) = sd.velocity_meter_data {
        return Some(StreamableData::VelocityMeter(decode_velocity_meter(d)));
    }
    if let Some(ref d) = sd.extension_data {
        if let Some(payload) = decode_extension_proto(&d.type_name, &d.payload) {
            return Some(StreamableData::Extension(ExtensionEnvelope::from_parts(
                d.type_name.clone(),
                d.sender_id.clone(),
                nanos_to_time(d.timestamp),
                payload,
            )));
        }
    }

    None
}

pub fn decode_with_seq(bytes: &[u8]) -> Option<(i32, StreamableData)> {
    let sd = proto::StreamData::decode(bytes).ok()?;
    let seq = sd.sequence_number;
    let data = decode(bytes)?;
    Some((seq, data))
}

// ---------------------------------------------------------------------------
// Per-type decode functions — field-for-field match with C++ ProtobufDecoder
// ---------------------------------------------------------------------------

fn decode_imu(d: &proto::ImuData) -> ImuData {
    ImuData {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        latency: d.latency,
        gyroscope: proto_to_vec3_opt(&d.gyroscope),
        accelerometer: proto_to_vec3_opt(&d.accelerometer),
        euler: proto_to_vec3_opt(&d.euler),
        quaternion: proto_to_quat_opt(&d.quaternion),
        linear_velocity: proto_to_vec3_opt(&d.linear_velocity),
        period: d.period,
        internal_frame_count: d.frame_count,
    }
}

fn decode_gnss(d: &proto::GnssData) -> GnssData {
    GnssData {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        latency: d.latency,
        latitude: d.latitude,
        longitude: d.longitude,
        height: d.height,
        horizontal_accuracy: d.horizontal_accuracy,
        vertical_accuracy: d.vertical_accuracy,
        quality: d.quality,
        n_sat: d.n_sat,
        hdop: d.hdop,
        tmg: d.tmg,
        heading: d.heading,
        altitude: d.altitude,
        undulation: d.undulation,
        orientation: proto_to_quat_opt(&d.orientation),
        period: d.period,
        internal_frame_count: d.frame_count,
        diff_age: d.diff_age,
    }
}

fn decode_optical(d: &proto::OpticalData) -> OpticalData {
    OpticalData {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        latency: d.latency,
        position: proto_to_vec3_opt(&d.position),
        orientation: proto_to_quat_opt(&d.orientation),
        angular_velocity: proto_to_vec3_opt(&d.angular_velocity),
        quality: d.quality,
        frame_rate: d.frame_rate,
        frame_number: d.frame_number,
        last_data_time: UNIX_EPOCH,
        interval: Duration::from_micros(11_111),
    }
}

fn decode_fused_pose(d: &proto::FusedPose) -> FusedPose {
    FusedPose {
        timestamp: nanos_to_time(d.timestamp),
        transmission_time: nanos_to_time(d.transmission_time),
        latency: d.latency,
        sender_id: d.sender_id.clone(),
        position: proto_to_vec3_opt(&d.position),
        orientation: proto_to_quat_opt(&d.orientation),
        angular_velocity: proto_to_vec3_opt(&d.angular_velocity),
        velocity: proto_to_vec3_opt(&d.velocity),
        acceleration: proto_to_vec3_opt(&d.acceleration),
        frame_number: d.frame_number as i64,
        last_data_time: UNIX_EPOCH,
    }
}

fn decode_rtcm(d: &proto::RtcmData) -> RTCMData {
    RTCMData {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        chunk: d.chunk.clone(),
        length: d.length,
    }
}

fn decode_can(d: &proto::CanData) -> CANData {
    CANData {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        is_extended: d.is_extended,
        id: d.id,
        data: d.data.clone(),
        length: d.length,
    }
}

fn decode_vehicle_state(d: &proto::VehicleState) -> VehicleState {
    VehicleState {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        wheel_base: d.wheel_base,
        track_width: d.track_width,
        steering_angle_l: d.steering_angle_l,
        steering_angle_r: d.steering_angle_r,
        steering_speed: 0.0,
        wheel_fr: d.wheel_fr,
        wheel_fl: d.wheel_fl,
        wheel_rr: d.wheel_rr,
        wheel_rl: d.wheel_rl,
        timestamp_ns: 0.0,
    }
}

fn decode_vehicle_speed(d: &proto::VehicleSpeed) -> VehicleSpeed {
    VehicleSpeed {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        linear: d.linear,
        angular: d.angular,
        valid_angular: d.valid_angular,
    }
}

fn decode_velocity_meter(d: &proto::VelocityMeterData) -> VelocityMeterData {
    VelocityMeterData {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        counter: d.counter,
        velocity: d.velocity,
        distance: d.distance,
        material: d.material,
        doppler_level: d.doppler_level,
        output_status: d.output_status,
    }
}

fn decode_fused_vehicle_pose(d: &proto::FusedVehiclePose) -> FusedVehiclePose {
    FusedVehiclePose {
        sender_id: String::new(),
        timestamp: nanos_to_time(d.timestamp),
        timecode: nanos_to_time(d.timecode),
        position: proto_to_vec2_opt(&d.position),
        yaw: d.yaw,
        utm_zone: d.utm_zone.clone(),
        global_position: proto_to_vec2_opt(&d.global_position),
        acceleration: proto_to_vec3_opt(&d.acceleration),
    }
}

fn decode_fused_vehicle_pose_v2(d: &proto::FusedVehiclePoseV2) -> FusedVehiclePoseV2 {
    FusedVehiclePoseV2 {
        timestamp: nanos_to_time(d.timestamp),
        transmission_time: nanos_to_time(d.transmission_time),
        sender_id: d.sender_id.clone(),
        position: proto_to_vec2_opt(&d.position),
        yaw: d.yaw,
        angular_velocity: d.angular_velocity,
        utm_zone: d.utm_zone.clone(),
        global_position: proto_to_vec2_opt(&d.global_position),
        velocity: proto_to_vec2_opt(&d.velocity),
        acceleration: proto_to_vec2_opt(&d.acceleration),
        internal_frame_count: d.internal_frame_count as i64,
        last_data_time: UNIX_EPOCH,
    }
}

fn decode_global_fused_pose(d: &proto::GlobalFusedPose) -> GlobalFusedPose {
    let pos = d.position.as_ref();
    GlobalFusedPose {
        timestamp: nanos_to_time(d.timestamp),
        transmission_time: nanos_to_time(d.transmission_time),
        sender_id: d.sender_id.clone(),
        position: GpsPoint {
            longitude: pos.map(|p| p.longitude).unwrap_or(0.0),
            latitude: pos.map(|p| p.latitude).unwrap_or(0.0),
            height: pos.map(|p| p.height).unwrap_or(0.0),
            timestamp: UNIX_EPOCH,
            sender_id: String::new(),
        },
        orientation: proto_to_quat_opt(&d.orientation),
        latency_in_ms: 0,
        last_data_time: UNIX_EPOCH,
    }
}

fn decode_fusion_state_int(d: &proto::FusionStateInt) -> FusionStateInt {
    FusionStateInt {
        timestamp: nanos_to_time(d.timestamp),
        sender_id: d.sender_id.clone(),
        position: proto_to_vec3_opt(&d.position),
        velocity: proto_to_vec3_opt(&d.velocity),
        gravity: d.gravity,
        imu_orientation: proto_to_quat_opt(&d.imu_orientation),
        omega_bias: proto_to_vec3_opt(&d.omega_bias),
        accel_bias: proto_to_vec3_opt(&d.accel_bias),
        imu_position: proto_to_vec3_opt(&d.imu_position),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_timestamp() -> SystemTime {
        UNIX_EPOCH + Duration::from_nanos(1_700_000_000_000_000_000)
    }

    #[test]
    fn roundtrip_imu() {
        let enc = ProtobufEncoder::new();
        let imu = ImuData {
            sender_id: "imu0".into(),
            timestamp: test_timestamp(),
            latency: 0.001,
            gyroscope: Vec3d::new(0.1, 0.2, 0.3),
            accelerometer: Vec3d::new(9.8, 0.0, 0.0),
            quaternion: UnitQuaternion::identity(),
            euler: Vec3d::new(0.0, 0.0, 0.0),
            period: 0.01,
            internal_frame_count: 42,
            linear_velocity: Vec3d::new(1.0, 2.0, 3.0),
        };
        let bytes = enc.encode(&StreamableData::Imu(imu.clone()));
        let decoded = decode(&bytes).unwrap();
        match decoded {
            StreamableData::Imu(d) => {
                assert_eq!(d.sender_id, "imu0");
                assert_eq!(d.timestamp, test_timestamp());
                assert!((d.latency - 0.001).abs() < 1e-15);
                assert!((d.gyroscope.x - 0.1).abs() < 1e-15);
                assert!((d.gyroscope.y - 0.2).abs() < 1e-15);
                assert!((d.gyroscope.z - 0.3).abs() < 1e-15);
                assert!((d.accelerometer.x - 9.8).abs() < 1e-15);
                assert_eq!(d.internal_frame_count, 42);
                assert!((d.period - 0.01).abs() < 1e-15);
                assert!((d.linear_velocity.x - 1.0).abs() < 1e-15);
            }
            _ => panic!("expected Imu"),
        }
    }

    #[test]
    fn roundtrip_gnss() {
        let enc = ProtobufEncoder::new();
        let gnss = GnssData {
            sender_id: "gnss0".into(),
            timestamp: test_timestamp(),
            latency: 0.05,
            latitude: 48.137154,
            longitude: 11.576124,
            height: 515.0,
            horizontal_accuracy: 1.5,
            vertical_accuracy: 2.5,
            altitude: 520.0,
            undulation: 5.0,
            quality: 4,
            n_sat: 12,
            hdop: 0.8,
            tmg: 45.0,
            heading: 90.0,
            period: 1.0,
            internal_frame_count: 100,
            orientation: UnitQuaternion::identity(),
            diff_age: 3.0,
        };
        let bytes = enc.encode(&StreamableData::Gnss(gnss.clone()));
        let decoded = decode(&bytes).unwrap();
        match decoded {
            StreamableData::Gnss(d) => {
                assert_eq!(d.sender_id, "gnss0");
                assert!((d.latitude - 48.137154).abs() < 1e-10);
                assert!((d.longitude - 11.576124).abs() < 1e-10);
                assert_eq!(d.quality, 4);
                assert_eq!(d.n_sat, 12);
                assert!((d.diff_age - 3.0).abs() < 1e-15);
            }
            _ => panic!("expected Gnss"),
        }
    }

    #[test]
    fn roundtrip_optical() {
        let enc = ProtobufEncoder::new();
        let opt = OpticalData {
            sender_id: "cam0".into(),
            timestamp: test_timestamp(),
            last_data_time: UNIX_EPOCH,
            latency: 0.002,
            position: Vec3d::new(1.0, 2.0, 3.0),
            orientation: UnitQuaternion::from_euler_angles(0.1, 0.2, 0.3),
            angular_velocity: Vec3d::new(0.01, 0.02, 0.03),
            quality: 0.95,
            frame_rate: 120.0,
            frame_number: 5000,
            interval: Duration::from_micros(8333),
        };
        let bytes = enc.encode(&StreamableData::Optical(opt.clone()));
        let decoded = decode(&bytes).unwrap();
        match decoded {
            StreamableData::Optical(d) => {
                assert_eq!(d.sender_id, "cam0");
                assert!((d.position.x - 1.0).abs() < 1e-15);
                assert!((d.quality - 0.95).abs() < 1e-15);
                assert!((d.frame_rate - 120.0).abs() < 1e-15);
                assert_eq!(d.frame_number, 5000);
                assert!((d.orientation.w - opt.orientation.w).abs() < 1e-10);
            }
            _ => panic!("expected Optical"),
        }
    }

    #[test]
    fn roundtrip_fused_pose() {
        let enc = ProtobufEncoder::new();
        let fp = FusedPose {
            sender_id: "fusion0".into(),
            timestamp: test_timestamp(),
            transmission_time: test_timestamp(),
            last_data_time: UNIX_EPOCH,
            position: Vec3d::new(10.0, 20.0, 30.0),
            orientation: UnitQuaternion::identity(),
            angular_velocity: Vec3d::new(0.1, 0.2, 0.3),
            velocity: Vec3d::new(1.0, 0.0, 0.0),
            acceleration: Vec3d::new(0.5, 0.0, 0.0),
            frame_number: 999,
            latency: 0.003,
        };
        let bytes = enc.encode(&StreamableData::FusedPose(fp.clone()));
        let decoded = decode(&bytes).unwrap();
        match decoded {
            StreamableData::FusedPose(d) => {
                assert_eq!(d.sender_id, "fusion0");
                assert!((d.position.x - 10.0).abs() < 1e-15);
                assert_eq!(d.frame_number, 999);
            }
            _ => panic!("expected FusedPose"),
        }
    }

    #[test]
    fn roundtrip_rtcm() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::Rtcm(RTCMData {
            sender_id: "ntrip0".into(),
            timestamp: test_timestamp(),
            chunk: vec![0xD3, 0x00, 0x13, 0xFF],
            length: 4,
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::Rtcm(d) => {
                assert_eq!(d.chunk, vec![0xD3, 0x00, 0x13, 0xFF]);
                assert_eq!(d.length, 4);
            }
            _ => panic!("expected Rtcm"),
        }
    }

    #[test]
    fn roundtrip_can() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::Can(CANData {
            sender_id: "can0".into(),
            timestamp: test_timestamp(),
            is_extended: true,
            id: 0x1A3,
            length: 8,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::Can(d) => {
                assert!(d.is_extended);
                assert_eq!(d.id, 0x1A3);
                assert_eq!(d.data, vec![1, 2, 3, 4, 5, 6, 7, 8]);
            }
            _ => panic!("expected Can"),
        }
    }

    #[test]
    fn roundtrip_vehicle_state() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::VehicleState(VehicleState {
            sender_id: "veh0".into(),
            timestamp: test_timestamp(),
            wheel_base: 2.7,
            track_width: 1.5,
            steering_angle_l: 0.1,
            steering_angle_r: 0.09,
            steering_speed: 0.0,
            wheel_fr: 10.0,
            wheel_fl: 10.1,
            wheel_rr: 10.2,
            wheel_rl: 10.3,
            timestamp_ns: 0.0,
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::VehicleState(d) => {
                assert!((d.wheel_base - 2.7).abs() < 1e-15);
                assert!((d.track_width - 1.5).abs() < 1e-15);
            }
            _ => panic!("expected VehicleState"),
        }
    }

    #[test]
    fn roundtrip_vehicle_speed() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::VehicleSpeed(VehicleSpeed {
            sender_id: "spd0".into(),
            timestamp: test_timestamp(),
            linear: 13.5,
            angular: 0.02,
            valid_angular: true,
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::VehicleSpeed(d) => {
                assert!((d.linear - 13.5).abs() < 1e-15);
                assert!(d.valid_angular);
            }
            _ => panic!("expected VehicleSpeed"),
        }
    }

    #[test]
    fn roundtrip_velocity_meter() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::VelocityMeter(VelocityMeterData {
            sender_id: "vm0".into(),
            timestamp: test_timestamp(),
            counter: 1000,
            velocity: 5.5,
            distance: 100.0,
            material: 0.8,
            doppler_level: 42.0,
            output_status: 1,
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::VelocityMeter(d) => {
                assert_eq!(d.counter, 1000);
                assert!((d.velocity - 5.5).abs() < 1e-15);
            }
            _ => panic!("expected VelocityMeter"),
        }
    }

    #[test]
    fn roundtrip_fused_vehicle_pose() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::FusedVehiclePose(FusedVehiclePose {
            sender_id: "fvp0".into(),
            timestamp: test_timestamp(),
            timecode: test_timestamp(),
            position: Vec2d::new(100.0, 200.0),
            yaw: 1.57,
            utm_zone: "32U".into(),
            global_position: Vec2d::new(11.5, 48.1),
            acceleration: Vec3d::new(0.1, 0.2, 0.3),
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::FusedVehiclePose(d) => {
                assert!((d.position.x - 100.0).abs() < 1e-15);
                assert_eq!(d.utm_zone, "32U");
            }
            _ => panic!("expected FusedVehiclePose"),
        }
    }

    #[test]
    fn roundtrip_fused_vehicle_pose_v2() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::FusedVehiclePoseV2(FusedVehiclePoseV2 {
            sender_id: "fvp2".into(),
            timestamp: test_timestamp(),
            transmission_time: test_timestamp(),
            last_data_time: UNIX_EPOCH,
            position: Vec2d::new(100.0, 200.0),
            yaw: 1.57,
            utm_zone: "32U".into(),
            global_position: Vec2d::new(11.5, 48.1),
            acceleration: Vec2d::new(0.1, 0.2),
            velocity: Vec2d::new(5.0, 0.0),
            angular_velocity: 0.05,
            internal_frame_count: 42,
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::FusedVehiclePoseV2(d) => {
                assert_eq!(d.sender_id, "fvp2");
                assert!((d.angular_velocity - 0.05).abs() < 1e-15);
                assert_eq!(d.internal_frame_count, 42);
            }
            _ => panic!("expected FusedVehiclePoseV2"),
        }
    }

    #[test]
    fn roundtrip_global_fused_pose() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::GlobalFusedPose(GlobalFusedPose {
            sender_id: "gfp0".into(),
            timestamp: test_timestamp(),
            transmission_time: test_timestamp(),
            last_data_time: UNIX_EPOCH,
            position: GpsPoint {
                longitude: 11.576124,
                latitude: 48.137154,
                height: 515.0,
                timestamp: UNIX_EPOCH,
                sender_id: String::new(),
            },
            orientation: UnitQuaternion::identity(),
            latency_in_ms: 0,
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::GlobalFusedPose(d) => {
                assert!((d.position.longitude - 11.576124).abs() < 1e-10);
                assert!((d.position.latitude - 48.137154).abs() < 1e-10);
            }
            _ => panic!("expected GlobalFusedPose"),
        }
    }

    #[test]
    fn roundtrip_fusion_state_int() {
        let enc = ProtobufEncoder::new();
        let data = StreamableData::FusionStateInt(FusionStateInt {
            sender_id: "fsi0".into(),
            timestamp: test_timestamp(),
            position: Vec3d::new(1.0, 2.0, 3.0),
            velocity: Vec3d::new(0.1, 0.2, 0.3),
            gravity: 9.81,
            imu_orientation: UnitQuaternion::identity(),
            omega_bias: Vec3d::new(0.001, 0.002, 0.003),
            accel_bias: Vec3d::new(0.01, 0.02, 0.03),
            imu_position: Vec3d::new(0.5, 0.0, 0.1),
        });
        let bytes = enc.encode(&data);
        match decode(&bytes).unwrap() {
            StreamableData::FusionStateInt(d) => {
                assert!((d.gravity - 9.81).abs() < 1e-15);
                assert!((d.position.x - 1.0).abs() < 1e-15);
            }
            _ => panic!("expected FusionStateInt"),
        }
    }

    #[test]
    fn sequence_numbers_increment() {
        let enc = ProtobufEncoder::new();
        let imu = StreamableData::Imu(ImuData::default());

        let bytes1 = enc.encode(&imu);
        let bytes2 = enc.encode(&imu);
        let bytes3 = enc.encode(&imu);

        let (seq1, _) = decode_with_seq(&bytes1).unwrap();
        let (seq2, _) = decode_with_seq(&bytes2).unwrap();
        let (seq3, _) = decode_with_seq(&bytes3).unwrap();

        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(seq3, 3);
    }

    #[test]
    fn decode_invalid_bytes_returns_none() {
        assert!(decode(&[0xFF, 0xFF, 0xFF]).is_none());
    }

    #[test]
    fn decode_empty_stream_data_returns_none() {
        let sd = proto::StreamData::default();
        let bytes = sd.encode_to_vec();
        assert!(decode(&bytes).is_none());
    }
}
