use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::Mutex;

use fusion_types::{FusedPose, ImuData, OpticalData, StreamableData, Vec3d};
use networking::{NetworkWriter, Subscriber};

/// C-compatible pose structure matching the C++ `Pose` in FusionHubCAPI.h.
/// Uses f32 to match the OVR data structure.
#[repr(C)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rw: f32,
    pub rx: f32,
    pub ry: f32,
    pub rz: f32,
    pub wx: f32,
    pub wy: f32,
    pub wz: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            rw: 1.0,
            rx: 0.0,
            ry: 0.0,
            rz: 0.0,
            wx: 0.0,
            wy: 0.0,
            wz: 0.0,
        }
    }
}

/// C-compatible IMU data structure.
#[repr(C)]
pub struct CImuData {
    pub timestamp: u64,
    pub wx: f64,
    pub wy: f64,
    pub wz: f64,
}

/// C-compatible optical data structure.
#[repr(C)]
pub struct COpticalData {
    pub timestamp: u64,
    pub px: f64,
    pub py: f64,
    pub pz: f64,
}

static LATEST_FUSED_POSE: Mutex<Option<FusedPose>> = Mutex::new(None);
static PREDICTION_TIME_MODIFIER: Mutex<f64> = Mutex::new(0.0);
static IMU_WRITER: Mutex<Option<NetworkWriter>> = Mutex::new(None);
static OPTICAL_WRITER: Mutex<Option<NetworkWriter>> = Mutex::new(None);

fn unsafe_c_str_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .unwrap_or("")
        .to_owned()
}

/// Initialize the CAPI module. Sets up logging.
#[no_mangle]
pub extern "C" fn init_fusion_hub_capi() {
    log_utils::init();
    log::info!("FusionHub C API initialized");
}

/// Subscribe to fused pose data from the given ZMQ endpoint.
/// Incoming `FusedPose` messages are stored in global state accessible via
/// `get_latest_fused_pose`.
#[no_mangle]
pub extern "C" fn subscribe_fused_pose(endpoint: *const c_char) {
    let ep = unsafe_c_str_to_string(endpoint);
    if ep.is_empty() {
        log::warn!("subscribe_fused_pose: empty endpoint");
        return;
    }

    log::info!("Subscribing to fused pose on {}", ep);
    let sub = Subscriber::new(vec![ep]);
    let _ = sub.start_listening(|data| {
        if let StreamableData::FusedPose(pose) = data {
            let mut guard = LATEST_FUSED_POSE.lock().unwrap();
            *guard = Some(pose);
        }
    });
}

/// Return the most recently received fused pose as a C `Pose` struct.
#[no_mangle]
pub extern "C" fn get_latest_fused_pose() -> Pose {
    let guard = LATEST_FUSED_POSE.lock().unwrap();
    match guard.as_ref() {
        Some(fp) => {
            let q = fp.orientation;
            Pose {
                x: fp.position.x as f32,
                y: fp.position.y as f32,
                z: fp.position.z as f32,
                rw: q.w as f32,
                rx: q.i as f32,
                ry: q.j as f32,
                rz: q.k as f32,
                wx: fp.angular_velocity.x as f32,
                wy: fp.angular_velocity.y as f32,
                wz: fp.angular_velocity.z as f32,
            }
        }
        None => Pose::default(),
    }
}

/// Create a ZMQ publisher for IMU data on the given endpoint.
#[no_mangle]
pub extern "C" fn publish_imu_data(endpoint: *const c_char) {
    let ep = unsafe_c_str_to_string(endpoint);
    if ep.is_empty() {
        log::warn!("publish_imu_data: empty endpoint");
        return;
    }
    log::info!("Publishing IMU data on {}", ep);
    let mut guard = IMU_WRITER.lock().unwrap();
    *guard = Some(NetworkWriter::new(ep));
}

/// Push a single IMU sample to the configured publisher.
#[no_mangle]
pub extern "C" fn push_imu_data(d: CImuData) {
    let guard = IMU_WRITER.lock().unwrap();
    if let Some(writer) = guard.as_ref() {
        let imu = ImuData {
            timestamp: std::time::UNIX_EPOCH + std::time::Duration::from_micros(d.timestamp),
            gyroscope: Vec3d::new(d.wx, d.wy, d.wz),
            ..ImuData::default()
        };
        let _ = writer.store(&StreamableData::Imu(imu));
    }
}

/// Create a ZMQ publisher for optical data on the given endpoint.
#[no_mangle]
pub extern "C" fn publish_optical_data(endpoint: *const c_char) {
    let ep = unsafe_c_str_to_string(endpoint);
    if ep.is_empty() {
        log::warn!("publish_optical_data: empty endpoint");
        return;
    }
    log::info!("Publishing optical data on {}", ep);
    let mut guard = OPTICAL_WRITER.lock().unwrap();
    *guard = Some(NetworkWriter::new(ep));
}

/// Push a single optical sample to the configured publisher.
#[no_mangle]
pub extern "C" fn push_optical_data(d: COpticalData) {
    let guard = OPTICAL_WRITER.lock().unwrap();
    if let Some(writer) = guard.as_ref() {
        let opt = OpticalData {
            timestamp: std::time::UNIX_EPOCH + std::time::Duration::from_micros(d.timestamp),
            position: Vec3d::new(d.px, d.py, d.pz),
            ..OpticalData::default()
        };
        let _ = writer.store(&StreamableData::Optical(opt));
    }
}

/// Get the current prediction time modifier value.
#[no_mangle]
pub extern "C" fn get_prediction_time_modifier() -> f64 {
    *PREDICTION_TIME_MODIFIER.lock().unwrap()
}

/// Set the prediction time modifier value.
#[no_mangle]
pub extern "C" fn set_prediction_time_modifier(t: f64) {
    *PREDICTION_TIME_MODIFIER.lock().unwrap() = t;
}

/// Subscribe to configuration commands from the given ZMQ endpoint.
#[no_mangle]
pub extern "C" fn subscribe_config_command(endpoint: *const c_char) {
    let ep = unsafe_c_str_to_string(endpoint);
    if ep.is_empty() {
        log::warn!("subscribe_config_command: empty endpoint");
        return;
    }
    log::info!("Subscribing to config commands on {}", ep);
    let sub = Subscriber::new(vec![ep]);
    let _ = sub.start_listening(|data| {
        log::debug!("Received config command data: {:?}", data);
    });
}

/// Rotate a vector (x, y, z) by a quaternion (w, rx, ry, rz).
///
/// The result is written back into the x, y, z pointers.
///
/// # Safety
/// All pointers must be non-null and aligned.
#[no_mangle]
pub unsafe extern "C" fn rotate_vector_by_quaternion(
    x: *mut f64,
    y: *mut f64,
    z: *mut f64,
    w: f64,
    rx: f64,
    ry: f64,
    rz: f64,
) {
    if x.is_null() || y.is_null() || z.is_null() {
        return;
    }
    let v = Vec3d::new(*x, *y, *z);
    let q = nalgebra::UnitQuaternion::from_quaternion(nalgebra::Quaternion::new(w, rx, ry, rz));
    let rotated = q * v;
    *x = rotated.x;
    *y = rotated.y;
    *z = rotated.z;
}

/// Invert a quaternion in-place.
///
/// # Safety
/// All pointers must be non-null and aligned.
#[no_mangle]
pub unsafe extern "C" fn invert_quaternion(
    w: *mut f64,
    rx: *mut f64,
    ry: *mut f64,
    rz: *mut f64,
) {
    if w.is_null() || rx.is_null() || ry.is_null() || rz.is_null() {
        return;
    }
    let q = nalgebra::UnitQuaternion::from_quaternion(nalgebra::Quaternion::new(*w, *rx, *ry, *rz));
    let inv = q.inverse();
    *w = inv.w;
    *rx = inv.i;
    *ry = inv.j;
    *rz = inv.k;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_default_identity_rotation() {
        let p = Pose::default();
        assert_eq!(p.rw, 1.0);
        assert_eq!(p.rx, 0.0);
        assert_eq!(p.x, 0.0);
    }

    #[test]
    fn get_latest_fused_pose_returns_default_when_empty() {
        let p = get_latest_fused_pose();
        assert_eq!(p.rw, 1.0);
        assert_eq!(p.x, 0.0);
    }

    #[test]
    fn prediction_time_modifier_roundtrip() {
        set_prediction_time_modifier(0.42);
        assert!((get_prediction_time_modifier() - 0.42).abs() < 1e-12);
        set_prediction_time_modifier(0.0);
    }

    #[test]
    fn rotate_vector_by_identity() {
        let mut x = 1.0_f64;
        let mut y = 2.0_f64;
        let mut z = 3.0_f64;
        unsafe {
            rotate_vector_by_quaternion(&mut x, &mut y, &mut z, 1.0, 0.0, 0.0, 0.0);
        }
        assert!((x - 1.0).abs() < 1e-12);
        assert!((y - 2.0).abs() < 1e-12);
        assert!((z - 3.0).abs() < 1e-12);
    }

    #[test]
    fn invert_identity_quaternion() {
        let mut w = 1.0_f64;
        let mut rx = 0.0_f64;
        let mut ry = 0.0_f64;
        let mut rz = 0.0_f64;
        unsafe {
            invert_quaternion(&mut w, &mut rx, &mut ry, &mut rz);
        }
        assert!((w - 1.0).abs() < 1e-12);
        assert!(rx.abs() < 1e-12);
    }
}
