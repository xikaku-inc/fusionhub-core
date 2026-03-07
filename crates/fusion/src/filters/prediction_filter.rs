use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use fusion_registry::{sf, SettingsField};
use fusion_types::{
    ApiRequest, FusedPose, FusedVehiclePose, FusedVehiclePoseV2, JsonValueExt, Quatd,
    StreamableData, Vec2d, Vec3d,
};
use nalgebra::{UnitQuaternion, Vector4};
use serde_json::json;

use crate::node::{CommandConsumerCallback, ConsumerCallback, Node};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("rotationInterval", "Rotation Interval (s)", "number", json!(0.0)),
        sf("positionInterval", "Position Interval (s)", "number", json!(0.0)),
        sf("predictPosition", "Predict Position", "boolean", json!(false)),
        sf("fixedPredictionInterval", "Fixed Prediction Interval", "boolean", json!(true)),
    ]
}

const D2R: f64 = std::f64::consts::PI / 180.0;

/// Prediction filter that integrates orientation (and optionally position)
/// forward by a configurable interval to compensate for system latency.
///
/// Faithfully matches the C++ PredictionFilter implementation.
///
/// Receives FusedPose or FusedVehiclePoseV2 and uses angular velocity to
/// predict a future orientation, compensating for system latency.
pub struct PredictionFilter {
    m_name: String,
    m_enabled: bool,
    m_predict_position: bool,
    m_fixed_prediction_interval: bool,
    m_dt_rot: f64,
    m_dt_pos: f64,
    m_predicted_fused_pose: FusedPose,
    m_on_output: Arc<Mutex<Option<ConsumerCallback>>>,
    m_on_command_output: Arc<Mutex<Option<CommandConsumerCallback>>>,
}

impl PredictionFilter {
    pub fn new(config: serde_json::Value) -> Self {
        let dt_rot = config.value_f64("rotationInterval", 0.0);
        let dt_pos = config.value_f64("positionInterval", 0.0);
        let predict_position = config.value_bool("predictPosition", false);
        let fixed_prediction_interval = config.value_bool("fixedPredictionInterval", true);

        Self {
            m_name: config.value_str("name", "PredictionFilter"),
            m_enabled: true,
            m_predict_position: predict_position,
            m_fixed_prediction_interval: fixed_prediction_interval,
            m_dt_rot: dt_rot,
            m_dt_pos: dt_pos,
            m_predicted_fused_pose: FusedPose::default(),
            m_on_output: Arc::new(Mutex::new(None)),
            m_on_command_output: Arc::new(Mutex::new(None)),
        }
    }

    pub fn name(&self) -> &str {
        &self.m_name
    }

    pub fn set_on_output(&self, callback: ConsumerCallback) {
        let mut on_output = self.m_on_output.lock().unwrap();
        *on_output = Some(callback);
    }

    pub fn set_on_command_output(&self, callback: CommandConsumerCallback) {
        let mut cb = self.m_on_command_output.lock().unwrap();
        *cb = Some(callback);
    }

    pub fn rotation_interval(&self) -> f64 {
        self.m_dt_rot
    }

    pub fn rotation_interval_ms(&self) -> f64 {
        self.m_dt_rot
    }

    pub fn get_fused_pose(&self) -> &FusedPose {
        &self.m_predicted_fused_pose
    }

    pub fn process_data(&mut self, data: StreamableData) {
        match data {
            StreamableData::FusedPose(pose) => {
                self.process_fused_pose(pose);
            }
            StreamableData::FusedVehiclePoseV2(pose) => {
                self.process_fused_vehicle_pose_v2(pose);
            }
            _ => {}
        }
    }

    fn process_fused_pose(&mut self, mut d: FusedPose) {
        if self.m_fixed_prediction_interval {
            let ms = (self.m_dt_rot * 1000.0) as i64;
            d.sender_id.push_str(&format!("@{}", ms));

            let omega_rad = d.angular_velocity * D2R;
            predict_simple_3d(
                &mut d.orientation,
                &omega_rad,
                &mut d.position,
                &d.velocity,
                &d.acceleration,
                self.m_dt_rot,
                self.m_dt_pos,
                self.m_predict_position,
            );
        } else {
            let dt = d.latency;
            d.sender_id.push_str("@latency");

            let omega_rad = d.angular_velocity * D2R;
            predict_simple_3d(
                &mut d.orientation,
                &omega_rad,
                &mut d.position,
                &d.velocity,
                &d.acceleration,
                dt,
                dt,
                self.m_predict_position,
            );
        }

        self.emit(StreamableData::FusedPose(d.clone()));
        self.m_predicted_fused_pose = d;
    }

    fn process_fused_vehicle_pose_v2(&mut self, mut d: FusedVehiclePoseV2) {
        let dt: f64;
        if self.m_fixed_prediction_interval {
            let ms = (self.m_dt_rot * 1000.0) as i64;
            d.sender_id.push_str(&format!("@{}", ms));
            // In fixed mode for vehicle V2, dt stays 0 (matching C++ behavior where
            // dt is default-initialized to 0 and never assigned in this branch)
            dt = 0.0;
        } else {
            // Compute latency as duration from transmissionTime to timestamp
            dt = duration_seconds(d.transmission_time, d.timestamp);
            d.sender_id.push_str("@latency");
        }

        // Convert yaw to 3D quaternion around Z axis
        let axis = nalgebra::Unit::new_normalize(Vec3d::new(0.0, 0.0, 1.0));
        let mut orientation3d = UnitQuaternion::from_axis_angle(&axis, d.yaw);
        let omega3d = Vec3d::new(0.0, 0.0, d.angular_velocity);
        let omega3d_rad = omega3d * D2R;

        // Predict local position
        predict_simple_2d(
            &mut orientation3d,
            &omega3d_rad,
            &mut d.position,
            &d.velocity,
            &d.acceleration,
            dt,
            dt,
            self.m_predict_position,
        );

        d.yaw = quaternion_to_yaw(&orientation3d);

        // Predict global position via UTM
        let (northing, easting, _utm_zone) =
            ll_to_utm(d.global_position.y, d.global_position.x);

        let mut global_pos = Vec2d::new(easting, northing);

        predict_simple_2d(
            &mut orientation3d,
            &omega3d_rad,
            &mut global_pos,
            &d.velocity,
            &d.acceleration,
            dt,
            dt,
            self.m_predict_position,
        );

        let (lat, lon) = utm_to_ll(global_pos.y, global_pos.x, &d.utm_zone);
        d.global_position = Vec2d::new(lon, lat);

        // Emit FusedVehiclePoseV2
        self.emit(StreamableData::FusedVehiclePoseV2(d.clone()));

        // Also emit FusedVehiclePose (converted from V2)
        let p_v1 = FusedVehiclePose::from(d);
        self.emit(StreamableData::FusedVehiclePose(p_v1));
    }

    pub fn process_command(&mut self, cmd: &ApiRequest) {
        if cmd.topic.contains(&self.m_name) {
            if cmd.command == "setConfigJsonPath" {
                if let Some(val) = cmd.data.get("rotationInterval").and_then(|v| v.as_f64()) {
                    self.m_dt_rot = val;
                } else {
                    log::warn!("Couldn't set prediction interval");
                }

                let return_data = serde_json::json!({
                    "rotationInterval": self.m_dt_rot
                });
                let req = ApiRequest::new(
                    cmd.command.clone(),
                    cmd.topic.clone(),
                    return_data,
                    cmd.id.clone(),
                );
                self.emit_command(req);
            }
        }
    }

    fn emit(&self, data: StreamableData) {
        let on_output = self.m_on_output.lock().unwrap();
        if let Some(ref callback) = *on_output {
            callback(data);
        }
    }

    fn emit_command(&self, cmd: ApiRequest) {
        let on_cmd = self.m_on_command_output.lock().unwrap();
        if let Some(ref callback) = *on_cmd {
            callback(cmd);
        }
    }
}

impl Node for PredictionFilter {
    fn name(&self) -> &str {
        &self.m_name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting PredictionFilter: {}", self.m_name);
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping PredictionFilter: {}", self.m_name);
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.m_enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.m_enabled = enabled;
    }

    fn receive_data(&mut self, data: StreamableData) {
        self.process_data(data);
    }

    fn receive_command(&mut self, cmd: &ApiRequest) {
        self.process_command(cmd);
    }

    fn set_on_output(&self, callback: ConsumerCallback) {
        self.set_on_output(callback);
    }

    fn set_on_command_output(&self, callback: CommandConsumerCallback) {
        self.set_on_command_output(callback);
    }
}

// ---------------------------------------------------------------------------
// Orientation integration (RK4) - matches C++ LP::IntegrateOrientation
// ---------------------------------------------------------------------------

type Mat4d = nalgebra::Matrix4<f64>;
type Vec4d = Vector4<f64>;

fn dqdt(gyr_rad_s: &Vec3d) -> Mat4d {
    let i = 0.5 * gyr_rad_s.x;
    let j = 0.5 * gyr_rad_s.y;
    let k = 0.5 * gyr_rad_s.z;

    // Matches C++ dqdt: right-multiplication 0.5 * (q * gyr)
    // Components in Eigen's xyzw order (nalgebra also uses [x,y,z,w] in .coords)
    Mat4d::new(
        0.0, k, -j, i, -k, 0.0, i, j, j, -i, 0.0, k, -i, -j, -k, 0.0,
    )
}

fn rk_step(
    q_in: &Quatd,
    h: f64,
    gyro_start: &Vec3d,
    gyro_halftime: &Vec3d,
    gyro_final: &Vec3d,
) -> Quatd {
    // nalgebra UnitQuaternion .coords gives [x, y, z, w] - same as Eigen's .coeffs()
    let q = q_in.as_ref().coords;

    let k1 = dqdt(gyro_start) * q;
    let k2 = dqdt(gyro_halftime) * (q + h / 2.0 * k1);
    let k3 = dqdt(gyro_halftime) * (q + h / 2.0 * k2);
    let k4 = dqdt(gyro_final) * (q + h * k3);

    let q_new = q + h / 6.0 * (k1 + 2.0 * k2 + 2.0 * k3 + k4);

    UnitQuaternion::new_normalize(nalgebra::Quaternion::from(q_new.normalize()))
}

/// Integrate orientation forward using body-frame angular velocity (rad/s)
/// with optional linear extrapolation of omega via omegaDot.
/// Matches C++ LP::IntegrateOrientation exactly (RK4 method).
fn integrate_orientation(
    n_seconds: f64,
    q_in: &Quatd,
    omega_rad_s: &Vec3d,
    omega_dot_rad_s2: &Vec3d,
) -> Quatd {
    let omega_half = omega_rad_s + n_seconds / 2.0 * omega_dot_rad_s2;
    let omega_end = omega_rad_s + n_seconds * omega_dot_rad_s2;

    rk_step(q_in, n_seconds, omega_rad_s, &omega_half, &omega_end)
}

// ---------------------------------------------------------------------------
// predictSimple - 3D variant (for FusedPose)
// ---------------------------------------------------------------------------

/// Matches C++ PredictionFilter::predictSimple with Vec3d position/velocity/acceleration.
fn predict_simple_3d(
    orientation: &mut Quatd,
    omega_rad: &Vec3d,
    position: &mut Vec3d,
    velocity: &Vec3d,
    acceleration: &Vec3d,
    dt_rot: f64,
    dt_pos: f64,
    predict_position: bool,
) {
    // Body-frame angular velocity: orientation.conjugate() * omega
    let body_omega = orientation.conjugate() * omega_rad;
    *orientation = integrate_orientation(dt_rot, orientation, &body_omega, &Vec3d::zeros());

    if predict_position {
        *position = *position + velocity * dt_pos + acceleration * (0.5 * dt_pos * dt_pos);
    }
}

// ---------------------------------------------------------------------------
// predictSimple - 2D variant (for FusedVehiclePoseV2)
// ---------------------------------------------------------------------------

/// Matches C++ PredictionFilter::predictSimple with Vec2d position/velocity/acceleration.
/// Orientation is still 3D quaternion; only position update is 2D.
fn predict_simple_2d(
    orientation: &mut Quatd,
    omega_rad: &Vec3d,
    position: &mut Vec2d,
    velocity: &Vec2d,
    acceleration: &Vec2d,
    dt_rot: f64,
    dt_pos: f64,
    predict_position: bool,
) {
    let body_omega = orientation.conjugate() * omega_rad;
    *orientation = integrate_orientation(dt_rot, orientation, &body_omega, &Vec3d::zeros());

    if predict_position {
        *position = *position + velocity * dt_pos + acceleration * (0.5 * dt_pos * dt_pos);
    }
}

// ---------------------------------------------------------------------------
// Quaternion ↔ Yaw helpers
// ---------------------------------------------------------------------------

fn quaternion_to_yaw(q: &Quatd) -> f64 {
    let siny_cosp = 2.0 * (q.w * q.k + q.i * q.j);
    let cosy_cosp = 1.0 - 2.0 * (q.j * q.j + q.k * q.k);
    siny_cosp.atan2(cosy_cosp)
}

// ---------------------------------------------------------------------------
// Duration helper
// ---------------------------------------------------------------------------

fn duration_seconds(from: SystemTime, to: SystemTime) -> f64 {
    match to.duration_since(from) {
        Ok(dur) => dur.as_secs_f64(),
        Err(e) => -(e.duration().as_secs_f64()),
    }
}

// ---------------------------------------------------------------------------
// UTM conversions (port of robot_localization navsat_conversions)
// ---------------------------------------------------------------------------

const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 0.0818191908 * 0.0818191908; // UTM_E2 = WGS84_E^2
const UTM_K0: f64 = 0.9996;

fn utm_letter_designator(lat: f64) -> char {
    if lat >= 72.0 && lat <= 84.0 {
        'X'
    } else if lat >= 64.0 {
        'W'
    } else if lat >= 56.0 {
        'V'
    } else if lat >= 48.0 {
        'U'
    } else if lat >= 40.0 {
        'T'
    } else if lat >= 32.0 {
        'S'
    } else if lat >= 24.0 {
        'R'
    } else if lat >= 16.0 {
        'Q'
    } else if lat >= 8.0 {
        'P'
    } else if lat >= 0.0 {
        'N'
    } else if lat >= -8.0 {
        'M'
    } else if lat >= -16.0 {
        'L'
    } else if lat >= -24.0 {
        'K'
    } else if lat >= -32.0 {
        'J'
    } else if lat >= -40.0 {
        'H'
    } else if lat >= -48.0 {
        'G'
    } else if lat >= -56.0 {
        'F'
    } else if lat >= -64.0 {
        'E'
    } else if lat >= -72.0 {
        'D'
    } else if lat >= -80.0 {
        'C'
    } else {
        'Z'
    }
}

/// Convert lat/lon (degrees) to UTM. Returns (northing, easting, zone_string).
fn ll_to_utm(lat: f64, lon: f64) -> (f64, f64, String) {
    let ecc_squared = WGS84_E2;
    let ecc4 = ecc_squared * ecc_squared;
    let ecc6 = ecc4 * ecc_squared;
    let k0 = UTM_K0;
    let a = WGS84_A;

    // Normalize longitude to -180..179.9
    let long_temp = (lon + 180.0) - ((lon + 180.0) / 360.0).floor() * 360.0 - 180.0;

    let lat_rad = lat * D2R;
    let long_rad = long_temp * D2R;

    let mut zone_number = ((long_temp + 180.0) / 6.0) as i32 + 1;

    // Special zone for southern Norway
    if lat >= 56.0 && lat < 64.0 && long_temp >= 3.0 && long_temp < 12.0 {
        zone_number = 32;
    }

    // Special zones for Svalbard
    if lat >= 72.0 && lat < 84.0 {
        if long_temp >= 0.0 && long_temp < 9.0 {
            zone_number = 31;
        } else if long_temp >= 9.0 && long_temp < 21.0 {
            zone_number = 33;
        } else if long_temp >= 21.0 && long_temp < 33.0 {
            zone_number = 35;
        } else if long_temp >= 33.0 && long_temp < 42.0 {
            zone_number = 37;
        }
    }

    let long_origin = (zone_number - 1) as f64 * 6.0 - 180.0 + 3.0;
    let long_origin_rad = long_origin * D2R;

    let utm_zone = format!("{}{}", zone_number, utm_letter_designator(lat));

    let ecc_prime_squared = ecc_squared / (1.0 - ecc_squared);

    let sin_lat = lat_rad.sin();
    let cos_lat = lat_rad.cos();
    let tan_lat = lat_rad.tan();

    let n = a / (1.0 - ecc_squared * sin_lat * sin_lat).sqrt();
    let t = tan_lat * tan_lat;
    let c = ecc_prime_squared * cos_lat * cos_lat;
    let a_val = cos_lat * (long_rad - long_origin_rad);

    let m = a
        * ((1.0 - ecc_squared / 4.0 - 3.0 * ecc4 / 64.0 - 5.0 * ecc6 / 256.0) * lat_rad
            - (3.0 * ecc_squared / 8.0 + 3.0 * ecc4 / 32.0 + 45.0 * ecc6 / 1024.0)
                * (2.0 * lat_rad).sin()
            + (15.0 * ecc4 / 256.0 + 45.0 * ecc6 / 1024.0) * (4.0 * lat_rad).sin()
            - (35.0 * ecc6 / 3072.0) * (6.0 * lat_rad).sin());

    let easting = k0 * n
        * (a_val
            + (1.0 - t + c) * a_val.powi(3) / 6.0
            + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ecc_prime_squared) * a_val.powi(5)
                / 120.0)
        + 500000.0;

    let mut northing = k0
        * (m + n * tan_lat
            * (a_val * a_val / 2.0
                + (5.0 - t + 9.0 * c + 4.0 * c * c) * a_val.powi(4) / 24.0
                + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ecc_prime_squared)
                    * a_val.powi(6)
                    / 720.0));

    if lat < 0.0 {
        northing += 10_000_000.0;
    }

    (northing, easting, utm_zone)
}

/// Convert UTM (northing, easting) to lat/lon (degrees) given a UTM zone string.
fn utm_to_ll(northing: f64, easting: f64, utm_zone: &str) -> (f64, f64) {
    let k0 = UTM_K0;
    let a = WGS84_A;
    let ecc_squared = WGS84_E2;
    let ecc_prime_squared = ecc_squared / (1.0 - ecc_squared);
    let e1 = (1.0 - (1.0 - ecc_squared).sqrt()) / (1.0 + (1.0 - ecc_squared).sqrt());

    let x = easting - 500000.0;
    let mut y = northing;

    // Parse zone number and letter
    let zone_letter_idx = utm_zone
        .find(|c: char| c.is_alphabetic())
        .unwrap_or(utm_zone.len());
    let zone_number: i32 = utm_zone[..zone_letter_idx].parse().unwrap_or(0);
    let zone_letter = utm_zone[zone_letter_idx..]
        .chars()
        .next()
        .unwrap_or('N');

    if (zone_letter as u8) < b'N' {
        y -= 10_000_000.0;
    }

    let long_origin = (zone_number - 1) as f64 * 6.0 - 180.0 + 3.0;

    let m_val = y / k0;
    let mu = m_val
        / (a
            * (1.0
                - ecc_squared / 4.0
                - 3.0 * ecc_squared.powi(2) / 64.0
                - 5.0 * ecc_squared.powi(3) / 256.0));

    let phi1_rad = mu
        + (3.0 * e1 / 2.0 - 27.0 * e1.powi(3) / 32.0) * (2.0 * mu).sin()
        + (21.0 * e1.powi(2) / 16.0 - 55.0 * e1.powi(4) / 32.0) * (4.0 * mu).sin()
        + (151.0 * e1.powi(3) / 96.0) * (6.0 * mu).sin();

    let sin_phi1 = phi1_rad.sin();
    let cos_phi1 = phi1_rad.cos();
    let tan_phi1 = phi1_rad.tan();

    let n1 = a / (1.0 - ecc_squared * sin_phi1 * sin_phi1).sqrt();
    let t1 = tan_phi1 * tan_phi1;
    let c1 = ecc_prime_squared * cos_phi1 * cos_phi1;
    let r1 = a * (1.0 - ecc_squared) / (1.0 - ecc_squared * sin_phi1 * sin_phi1).powf(1.5);
    let d = x / (n1 * k0);

    let lat = phi1_rad
        - (n1 * tan_phi1 / r1)
            * (d * d / 2.0
                - (5.0 + 3.0 * t1 + 10.0 * c1 - 4.0 * c1 * c1 - 9.0 * ecc_prime_squared)
                    * d.powi(4)
                    / 24.0
                + (61.0 + 90.0 * t1 + 298.0 * c1 + 45.0 * t1 * t1
                    - 252.0 * ecc_prime_squared
                    - 3.0 * c1 * c1)
                    * d.powi(6)
                    / 720.0);

    let lat_deg = lat / D2R;

    let lon = (d
        - (1.0 + 2.0 * t1 + c1) * d.powi(3) / 6.0
        + (5.0 - 2.0 * c1 + 28.0 * t1 - 3.0 * c1 * c1 + 8.0 * ecc_prime_squared
            + 24.0 * t1 * t1)
            * d.powi(5)
            / 120.0)
        / cos_phi1;

    let lon_deg = long_origin + lon / D2R;

    (lat_deg, lon_deg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn default_config() {
        let filter = PredictionFilter::new(serde_json::json!({}));
        assert_eq!(filter.rotation_interval_ms(), 0.0);
    }

    #[test]
    fn prediction_with_zero_interval() {
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
            assert!(angle < 1e-6);
        } else {
            panic!("expected FusedPose");
        }
    }

    #[test]
    fn prediction_rotates_orientation() {
        let mut filter = PredictionFilter::new(serde_json::json!({"rotationInterval": 0.1}));

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
            // 90 deg/s for 0.1 s = 9 degrees = ~0.157 rad
            assert!((angle - 0.157).abs() < 0.01);
        } else {
            panic!("expected FusedPose");
        }
    }

    #[test]
    fn process_command_sets_interval() {
        let mut filter = PredictionFilter::new(serde_json::json!({"name": "pred1"}));
        let cmd = ApiRequest::new(
            "setConfigJsonPath",
            "pred1",
            serde_json::json!({"rotationInterval": 0.05}),
            "1",
        );
        filter.process_command(&cmd);
        assert!((filter.rotation_interval() - 0.05).abs() < 1e-12);
    }

    #[test]
    fn process_command_emits_response() {
        let mut filter = PredictionFilter::new(serde_json::json!({"name": "pred1"}));

        let cmds = Arc::new(Mutex::new(Vec::new()));
        let c = cmds.clone();
        filter.set_on_command_output(Box::new(move |req| {
            c.lock().unwrap().push(req);
        }));

        let cmd = ApiRequest::new(
            "setConfigJsonPath",
            "pred1",
            serde_json::json!({"rotationInterval": 0.123}),
            "42",
        );
        filter.process_command(&cmd);

        let sent = cmds.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].command, "setConfigJsonPath");
        let rot = sent[0].data.get("rotationInterval").unwrap().as_f64().unwrap();
        assert!((rot - 0.123).abs() < 1e-12);
    }

    #[test]
    fn fixed_mode_sender_id_decoration() {
        let mut filter = PredictionFilter::new(serde_json::json!({
            "rotationInterval": 0.015
        }));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = FusedPose {
            sender_id: "fusion0".to_string(),
            ..Default::default()
        };
        filter.process_data(StreamableData::FusedPose(pose));

        let output = results.lock().unwrap();
        if let StreamableData::FusedPose(ref p) = output[0] {
            assert_eq!(p.sender_id, "fusion0@15");
        } else {
            panic!("expected FusedPose");
        }
    }

    #[test]
    fn latency_mode_sender_id() {
        let mut filter = PredictionFilter::new(serde_json::json!({
            "fixedPredictionInterval": false
        }));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = FusedPose {
            sender_id: "fusion0".to_string(),
            latency: 0.02,
            ..Default::default()
        };
        filter.process_data(StreamableData::FusedPose(pose));

        let output = results.lock().unwrap();
        if let StreamableData::FusedPose(ref p) = output[0] {
            assert_eq!(p.sender_id, "fusion0@latency");
        } else {
            panic!("expected FusedPose");
        }
    }

    #[test]
    fn predict_position_disabled_by_default() {
        let mut filter = PredictionFilter::new(serde_json::json!({
            "rotationInterval": 0.1,
            "positionInterval": 0.1
        }));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = FusedPose {
            position: Vec3d::new(1.0, 2.0, 3.0),
            velocity: Vec3d::new(10.0, 0.0, 0.0),
            ..Default::default()
        };
        filter.process_data(StreamableData::FusedPose(pose));

        let output = results.lock().unwrap();
        if let StreamableData::FusedPose(ref p) = output[0] {
            // Position should NOT change because predictPosition is false
            assert!((p.position.x - 1.0).abs() < 1e-12);
            assert!((p.position.y - 2.0).abs() < 1e-12);
            assert!((p.position.z - 3.0).abs() < 1e-12);
        } else {
            panic!("expected FusedPose");
        }
    }

    #[test]
    fn predict_position_enabled() {
        let mut filter = PredictionFilter::new(serde_json::json!({
            "rotationInterval": 0.1,
            "positionInterval": 0.2,
            "predictPosition": true
        }));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = FusedPose {
            position: Vec3d::new(0.0, 0.0, 0.0),
            velocity: Vec3d::new(10.0, 0.0, 0.0),
            acceleration: Vec3d::new(2.0, 0.0, 0.0),
            ..Default::default()
        };
        filter.process_data(StreamableData::FusedPose(pose));

        let output = results.lock().unwrap();
        if let StreamableData::FusedPose(ref p) = output[0] {
            // position = 0 + 10*0.2 + 2*0.5*0.2^2 = 2.0 + 0.04 = 2.04
            let dt_pos = 0.2;
            let expected_x = 10.0 * dt_pos + 2.0 * 0.5 * dt_pos * dt_pos;
            assert!((p.position.x - expected_x).abs() < 1e-10);
        } else {
            panic!("expected FusedPose");
        }
    }

    #[test]
    fn vehicle_pose_v2_fixed_mode() {
        // In fixed mode for vehicle V2, dt=0 so no prediction occurs
        let mut filter = PredictionFilter::new(serde_json::json!({
            "rotationInterval": 0.05,
            "predictPosition": true
        }));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = FusedVehiclePoseV2 {
            sender_id: "veh0".to_string(),
            yaw: 0.5,
            angular_velocity: 10.0,
            position: Vec2d::new(1.0, 2.0),
            velocity: Vec2d::new(5.0, 0.0),
            ..Default::default()
        };
        filter.process_data(StreamableData::FusedVehiclePoseV2(pose));

        let output = results.lock().unwrap();
        // Should emit 2 items: FusedVehiclePoseV2 + FusedVehiclePose
        assert_eq!(output.len(), 2);

        if let StreamableData::FusedVehiclePoseV2(ref p) = output[0] {
            assert_eq!(p.sender_id, "veh0@50");
            // dt=0 in fixed mode, so yaw and position should be unchanged
            assert!((p.yaw - 0.5).abs() < 1e-6);
            assert!((p.position.x - 1.0).abs() < 1e-6);
            assert!((p.position.y - 2.0).abs() < 1e-6);
        } else {
            panic!("expected FusedVehiclePoseV2, got {:?}", output[0]);
        }

        assert!(matches!(output[1], StreamableData::FusedVehiclePose(_)));
    }

    #[test]
    fn predicted_fused_pose_stored() {
        let mut filter = PredictionFilter::new(serde_json::json!({
            "rotationInterval": 0.1
        }));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = FusedPose {
            sender_id: "test".to_string(),
            angular_velocity: Vec3d::new(0.0, 0.0, 45.0),
            ..Default::default()
        };
        filter.process_data(StreamableData::FusedPose(pose));

        let stored = filter.get_fused_pose();
        assert!(stored.sender_id.starts_with("test@"));
    }

    #[test]
    fn body_frame_omega_conversion() {
        // When the current orientation is rotated 90 degrees around Z,
        // a world-frame omega of (90, 0, 0) deg/s should become body-frame (0, -90, 0).
        // After integration, the resulting orientation should reflect rotation around body Y.
        let mut filter = PredictionFilter::new(serde_json::json!({
            "rotationInterval": 0.1
        }));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let axis_z = nalgebra::Unit::new_normalize(Vec3d::new(0.0, 0.0, 1.0));
        let q_90z = UnitQuaternion::from_axis_angle(&axis_z, std::f64::consts::FRAC_PI_2);

        let pose = FusedPose {
            orientation: q_90z,
            angular_velocity: Vec3d::new(90.0, 0.0, 0.0),
            ..Default::default()
        };
        filter.process_data(StreamableData::FusedPose(pose));

        let output = results.lock().unwrap();
        if let StreamableData::FusedPose(ref p) = output[0] {
            // The orientation should have changed from q_90z
            let angle = p.orientation.angle_to(&q_90z);
            // 90 deg/s * 0.1 s = 9 degrees body frame rotation
            assert!((angle - 9.0 * D2R).abs() < 0.01);
        } else {
            panic!("expected FusedPose");
        }
    }

    #[test]
    fn utm_roundtrip() {
        let lat_in = 48.1351;
        let lon_in = 11.582;

        let (northing, easting, zone) = ll_to_utm(lat_in, lon_in);
        assert!(!zone.is_empty());

        let (lat_out, lon_out) = utm_to_ll(northing, easting, &zone);
        assert!(
            (lat_out - lat_in).abs() < 1e-6,
            "lat: {} vs {}",
            lat_out,
            lat_in
        );
        assert!(
            (lon_out - lon_in).abs() < 1e-6,
            "lon: {} vs {}",
            lon_out,
            lon_in
        );
    }

    #[test]
    fn quaternion_to_yaw_identity() {
        let q = Quatd::identity();
        let yaw = quaternion_to_yaw(&q);
        assert!(yaw.abs() < 1e-12);
    }

    #[test]
    fn quaternion_to_yaw_90_degrees() {
        let axis = nalgebra::Unit::new_normalize(Vec3d::new(0.0, 0.0, 1.0));
        let q = UnitQuaternion::from_axis_angle(&axis, std::f64::consts::FRAC_PI_2);
        let yaw = quaternion_to_yaw(&q);
        assert!((yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
    }

    #[test]
    fn integrate_orientation_matches_rk4() {
        // 90 deg/s around Z for 0.1s = 9 degrees
        let q = Quatd::identity();
        let omega = Vec3d::new(0.0, 0.0, 90.0 * D2R);
        let result = integrate_orientation(0.1, &q, &omega, &Vec3d::zeros());
        let angle = result.angle_to(&q);
        assert!((angle - 9.0 * D2R).abs() < 1e-6);
    }
}
