use std::collections::HashMap;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nalgebra::{Isometry3, Matrix3, Rotation3, Translation3, UnitQuaternion, Vector3};
use tokio::task::JoinHandle;

use fusion_registry::{sf, SettingsField};
use fusion_types::{JsonValueExt, OpticalData, StreamableData, Vec3d};
use serde_json::json;

use crate::node::{ConsumerCallback, Node, NodeBase};

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("port", "Port", "number", json!(5000)),
        sf("multicastGroup", "Multicast Group", "string", json!("")),
        sf("timeout", "Timeout (ms)", "number", json!(100)),
        sf("roomDirections", "Room Directions", "json", json!({})),
        sf("bodyDirections", "Body Directions", "json", json!({})),
        sf("roomOffset", "Room Offset", "vector3", json!({"x": 0, "y": 0, "z": 0})),
        sf("worldTrafo", "World Transform", "json", json!({})),
        sf("refToOpticalQuat", "Ref-to-Optical Rotation", "quaternion", json!({"w": 1, "x": 0, "y": 0, "z": 0})),
        sf("refToOpticalVec", "Ref-to-Optical Translation", "vector3", json!({"x": 0, "y": 0, "z": 0})),
    ]
}

// ---------------------------------------------------------------------------
// Coordinate frame utilities
// ---------------------------------------------------------------------------

/// Map a direction string to its axis vector in the SteamVR internal frame
/// (X = right, Y = up, Z = backward).
fn vec_for_direction(dir: &str) -> Option<Vector3<f64>> {
    match dir {
        "right" => Some(Vector3::x()),
        "left" => Some(-Vector3::x()),
        "up" => Some(Vector3::y()),
        "down" => Some(-Vector3::y()),
        "backward" => Some(Vector3::z()),
        "forward" => Some(-Vector3::z()),
        _ => None,
    }
}

/// Build a rotation matrix from three direction strings that describe
/// the X, Y, Z axes of a coordinate frame, then return the rotation
/// that transforms from the SteamVR frame to that frame (transposed).
///
/// This mirrors the C++ `RotationForDirections` function:
///   frameSteamVR().getRotationToFrame(frame).transpose()
///
/// Since SteamVR = identity, `getRotationToFrame(frame) = frame.m_toInternal^T * I`
/// and transposing that gives `frame.m_toInternal`, which is the matrix whose
/// columns are the axis vectors.
fn rotation_for_directions(directions: &[String; 3]) -> Result<Matrix3<f64>, String> {
    let vx = vec_for_direction(&directions[0])
        .ok_or_else(|| format!("Invalid direction: {}", directions[0]))?;
    let vy = vec_for_direction(&directions[1])
        .ok_or_else(|| format!("Invalid direction: {}", directions[1]))?;
    let vz = vec_for_direction(&directions[2])
        .ok_or_else(|| format!("Invalid direction: {}", directions[2]))?;

    // Build the "toInternal" matrix (columns are the axis vectors).
    let m = Matrix3::from_columns(&[vx, vy, vz]);

    // Check orthogonality.
    let diff = m * m.transpose() - Matrix3::identity();
    if diff.norm() > 1e-6 {
        return Err(
            "Directions don't give orthogonal frame composed of \
             'right', 'left', 'up', 'down', 'forward', 'backward'"
                .to_string(),
        );
    }

    // C++ returns: frameSteamVR().getRotationToFrame(frame).transpose()
    // = (frame.m_toInternal^T * steamvr.m_toInternal).transpose()
    // SteamVR m_toInternal = Identity, so this is:
    // (frame.m_toInternal^T)^T = frame.m_toInternal = m
    Ok(m)
}

// ---------------------------------------------------------------------------
// DTrack UDP packet parsing
// ---------------------------------------------------------------------------

/// A single 6DOF body parsed from a DTrack `6d` line.
#[derive(Debug, Clone)]
struct DTrackBody {
    id: i32,
    quality: f64,
    loc: [f64; 3],
    rot: [f64; 9],
}

/// Parsed content of a single DTrack UDP frame.
#[derive(Debug, Clone, Default)]
struct DTrackFrame {
    frame_number: Option<i64>,
    timestamp: Option<f64>,
    bodies: Vec<DTrackBody>,
}

/// Parse a complete DTrack ASCII UDP packet into a `DTrackFrame`.
fn parse_dtrack_packet(data: &str) -> DTrackFrame {
    let mut frame = DTrackFrame::default();

    for line in data.lines() {
        let line = line.trim();
        if line.starts_with("fr ") {
            if let Ok(n) = line[3..].trim().parse::<i64>() {
                frame.frame_number = Some(n);
            }
        } else if line.starts_with("ts ") {
            if let Ok(ts) = line[3..].trim().parse::<f64>() {
                frame.timestamp = Some(ts);
            }
        } else if line.starts_with("6d ") {
            frame.bodies = parse_6d_line(&line[3..]);
        }
    }

    frame
}

/// Parse the body portion of a `6d` line: `<count> [id quality][x y z][r0..r8] ...`
fn parse_6d_line(s: &str) -> Vec<DTrackBody> {
    let s = s.trim();
    let mut bodies = Vec::new();

    // Find where the first bracket starts; everything before that is the count.
    let bracket_start = match s.find('[') {
        Some(i) => i,
        None => return bodies,
    };

    let count_str = s[..bracket_start].trim();
    let count: usize = match count_str.parse() {
        Ok(c) => c,
        Err(_) => return bodies,
    };

    // Extract all bracket groups.
    let brackets = extract_bracket_groups(&s[bracket_start..]);

    // Each body needs 3 bracket groups: [id quality], [x y z], [r0..r8]
    if brackets.len() < count * 3 {
        log::warn!(
            "6d line: expected {} bracket groups for {} bodies, got {}",
            count * 3,
            count,
            brackets.len()
        );
    }

    for i in 0..count {
        let base = i * 3;
        if base + 2 >= brackets.len() {
            break;
        }

        let id_quality = &brackets[base];
        let position = &brackets[base + 1];
        let rotation = &brackets[base + 2];

        let iq: Vec<f64> = parse_floats(id_quality);
        let pos: Vec<f64> = parse_floats(position);
        let rot: Vec<f64> = parse_floats(rotation);

        if iq.len() < 2 || pos.len() < 3 || rot.len() < 9 {
            log::warn!("6d body {}: incomplete data", i);
            continue;
        }

        bodies.push(DTrackBody {
            id: iq[0] as i32,
            quality: iq[1],
            loc: [pos[0], pos[1], pos[2]],
            rot: [
                rot[0], rot[1], rot[2], rot[3], rot[4], rot[5], rot[6], rot[7], rot[8],
            ],
        });
    }

    bodies
}

/// Extract all `[...]` groups from the string. Does not handle nested brackets.
fn extract_bracket_groups(s: &str) -> Vec<String> {
    let mut groups = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, ch) in s.char_indices() {
        match ch {
            '[' => {
                if depth == 0 {
                    start = i + 1;
                }
                depth += 1;
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    groups.push(s[start..i].to_string());
                }
            }
            _ => {}
        }
    }

    groups
}

/// Parse a whitespace-separated list of floats.
fn parse_floats(s: &str) -> Vec<f64> {
    s.split_whitespace()
        .filter_map(|tok| tok.parse::<f64>().ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Midnight computation for latency
// ---------------------------------------------------------------------------

/// Compute UTC midnight for the given time point.
/// DTrack timestamps are seconds since UTC midnight, so we need midnight
/// to convert the timestamp to an absolute time.
fn compute_midnight_utc(now: SystemTime) -> SystemTime {
    let since_epoch = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = since_epoch.as_secs();
    let secs_today = secs % 86400;
    now - Duration::from_secs(secs_today)
}

// ---------------------------------------------------------------------------
// Parse JSON config helpers
// ---------------------------------------------------------------------------

fn parse_directions(config: &serde_json::Value, key: &str) -> [String; 3] {
    let default = ["right".to_string(), "up".to_string(), "backward".to_string()];
    config
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            if arr.len() == 3 {
                [
                    arr[0].as_str().unwrap_or("right").to_string(),
                    arr[1].as_str().unwrap_or("up").to_string(),
                    arr[2].as_str().unwrap_or("backward").to_string(),
                ]
            } else {
                default.clone()
            }
        })
        .unwrap_or(default)
}

/// Parse a quaternion from JSON array `[w, x, y, z]` (Eigen convention).
fn parse_quat(config: &serde_json::Value, key: &str) -> UnitQuaternion<f64> {
    config
        .get(key)
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            if arr.len() == 4 {
                let w = arr[0].as_f64()?;
                let x = arr[1].as_f64()?;
                let y = arr[2].as_f64()?;
                let z = arr[3].as_f64()?;
                Some(
                    UnitQuaternion::from_quaternion(nalgebra::Quaternion::new(w, x, y, z))
                )
            } else {
                None
            }
        })
        .unwrap_or_else(UnitQuaternion::identity)
}

/// Parse a Vec3d from JSON array `[x, y, z]`.
fn parse_vec3(config: &serde_json::Value, key: &str) -> Vector3<f64> {
    config
        .get(key)
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            if arr.len() == 3 {
                let x = arr[0].as_f64()?;
                let y = arr[1].as_f64()?;
                let z = arr[2].as_f64()?;
                Some(Vector3::new(x, y, z))
            } else {
                None
            }
        })
        .unwrap_or_else(Vector3::zeros)
}

// ---------------------------------------------------------------------------
// DTrackOpticalSource
// ---------------------------------------------------------------------------

/// DTrack optical tracking source node.
///
/// Receives 6DOF body tracking data from an ART DTrack system via UDP.
/// Parses the DTrack ASCII protocol directly without requiring the DTrack SDK.
pub struct DTrackOpticalSource {
    pub base: NodeBase,
    // Config values parsed at construction time.
    m_body_ids: Vec<i32>,          // 0-based (config is 1-based)
    m_port: u16,
    m_multicast_group: String,
    m_timeout: Duration,
    m_world_trafo: Isometry3<f64>,
    m_body_trafo: Matrix3<f64>,    // rotation-only body transform
    m_ref_to_optical_quat: UnitQuaternion<f64>,
    m_ref_to_optical_vec: Vector3<f64>,
    // Worker state.
    m_done: Arc<AtomicBool>,
    m_worker_handle: Option<JoinHandle<()>>,
}

impl DTrackOpticalSource {
    pub fn new(name: impl Into<String>, config: &serde_json::Value) -> Self {
        let name = name.into();

        // Body IDs: config is 1-based, convert to 0-based for filtering.
        let body_ids: Vec<i32> = config
            .get("bodyIDs")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64().map(|id| (id - 1) as i32))
                    .collect()
            })
            .unwrap_or_default();

        let port = config.value_u16("port", 5000);

        let multicast_group = config.value_str("multicastGroup", "");

        let timeout = Duration::from_millis(config.value_u64("timeout", 100));

        // Coordinate transforms.
        let room_directions = parse_directions(config, "roomDirections");
        let body_directions = parse_directions(config, "bodyDirections");

        // Build world trafo: room rotation * identity.
        let mut world_trafo = Isometry3::<f64>::identity();
        match rotation_for_directions(&room_directions) {
            Ok(rot_mat) => {
                let rotation = Rotation3::from_matrix_unchecked(rot_mat);
                world_trafo =
                    Isometry3::from_parts(Translation3::identity(), UnitQuaternion::from_rotation_matrix(&rotation));
            }
            Err(e) => {
                log::warn!("Cannot parse roomDirections: {}", e);
            }
        }

        // Add room offset to translation.
        let room_offset = parse_vec3(config, "roomOffset");
        world_trafo = Isometry3::from_parts(
            Translation3::from(world_trafo.translation.vector + room_offset),
            world_trafo.rotation,
        );

        // Build body trafo (rotation only).
        let mut body_trafo = Matrix3::identity();
        match rotation_for_directions(&body_directions) {
            Ok(rot_mat) => {
                body_trafo = rot_mat;
            }
            Err(e) => {
                log::warn!("Cannot parse bodyDirections: {}", e);
            }
        }

        // If an explicit worldTrafo is given, it overrides room directions and offset.
        if let Some(wt) = config.get("worldTrafo") {
            if world_trafo.rotation != UnitQuaternion::identity()
                || world_trafo.translation.vector != Vector3::zeros()
            {
                log::info!("Setting 'worldTrafo' overrides other coordinate transformations");
            }
            // Parse worldTrafo as { "quat": [w,x,y,z], "vect": [x,y,z] }
            let quat = wt
                .get("quat")
                .map(|q| {
                    if let Some(arr) = q.as_array() {
                        if arr.len() == 4 {
                            let w = arr[0].as_f64().unwrap_or(1.0);
                            let x = arr[1].as_f64().unwrap_or(0.0);
                            let y = arr[2].as_f64().unwrap_or(0.0);
                            let z = arr[3].as_f64().unwrap_or(0.0);
                            return UnitQuaternion::from_quaternion(
                                nalgebra::Quaternion::new(w, x, y, z),
                            );
                        }
                    }
                    // Object form: { "w": .., "x": .., "y": .., "z": .. }
                    let w = q.value_f64("w", 1.0);
                    let x = q.value_f64("x", 0.0);
                    let y = q.value_f64("y", 0.0);
                    let z = q.value_f64("z", 0.0);
                    UnitQuaternion::from_quaternion(nalgebra::Quaternion::new(w, x, y, z))
                })
                .unwrap_or_else(UnitQuaternion::identity);

            let vect = wt
                .get("vect")
                .map(|v| {
                    if let Some(arr) = v.as_array() {
                        if arr.len() == 3 {
                            return Vector3::new(
                                arr[0].as_f64().unwrap_or(0.0),
                                arr[1].as_f64().unwrap_or(0.0),
                                arr[2].as_f64().unwrap_or(0.0),
                            );
                        }
                    }
                    let x = v.value_f64("x", 0.0);
                    let y = v.value_f64("y", 0.0);
                    let z = v.value_f64("z", 0.0);
                    Vector3::new(x, y, z)
                })
                .unwrap_or_else(Vector3::zeros);

            world_trafo = Isometry3::from_parts(Translation3::from(vect), quat);
        }

        // Reference-to-optical-frame correction.
        let ref_to_optical_quat = parse_quat(config, "referenceToOpticalFrameQuat");
        let ref_to_optical_vec = parse_vec3(config, "referenceToOpticalFrameVec");

        log::info!(
            "DTrackOpticalSource '{}': port={}, multicast='{}', bodyIDs(0-based)={:?}",
            name,
            port,
            multicast_group,
            body_ids
        );
        log::info!(
            "  referenceToOpticalFrameQuat = {:?} (conjugate)",
            ref_to_optical_quat.conjugate()
        );
        log::info!(
            "  referenceToOpticalFrameVec = {:?}",
            ref_to_optical_vec
        );

        Self {
            base: NodeBase::new(&name),
            m_body_ids: body_ids,
            m_port: port,
            m_multicast_group: multicast_group,
            m_timeout: timeout,
            m_world_trafo: world_trafo,
            m_body_trafo: body_trafo,
            m_ref_to_optical_quat: ref_to_optical_quat,
            m_ref_to_optical_vec: ref_to_optical_vec,
            m_done: Arc::new(AtomicBool::new(false)),
            m_worker_handle: None,
        }
    }

    /// Convert DTrack position (mm) to meters and apply world transform.
    fn convert_location(world_trafo: &Isometry3<f64>, loc: &[f64; 3]) -> Vector3<f64> {
        let pos_m = Vector3::new(loc[0], loc[1], loc[2]) / 1000.0;
        world_trafo * pos_m
    }

    /// Convert DTrack column-major rotation matrix to quaternion,
    /// applying world and body transforms.
    fn convert_rotation(
        world_trafo: &Isometry3<f64>,
        body_trafo: &Matrix3<f64>,
        rot: &[f64; 9],
    ) -> UnitQuaternion<f64> {
        // DTrack rotation is column-major.
        #[rustfmt::skip]
        let col_major = Matrix3::new(
            rot[0], rot[3], rot[6],
            rot[1], rot[4], rot[7],
            rot[2], rot[5], rot[8],
        );

        let world_rot = world_trafo.rotation.to_rotation_matrix();
        let combined = world_rot.matrix() * col_major * body_trafo;
        UnitQuaternion::from_rotation_matrix(&Rotation3::from_matrix_unchecked(combined))
    }

    /// Apply the reference-to-optical-frame correction.
    /// pose = s_T_c * c_T_h, where c_T_h is the optical frame offset.
    fn correct_pose(
        position: Vector3<f64>,
        orientation: UnitQuaternion<f64>,
        ref_quat: &UnitQuaternion<f64>,
        ref_vec: &Vector3<f64>,
    ) -> (Vector3<f64>, UnitQuaternion<f64>) {
        let s_t_c = Isometry3::from_parts(
            Translation3::from(position),
            orientation,
        );
        let c_t_h = Isometry3::from_parts(
            Translation3::from(*ref_vec),
            *ref_quat,
        );
        let s_t_h = s_t_c * c_t_h;
        (
            s_t_h.translation.vector,
            s_t_h.rotation,
        )
    }
}

impl Node for DTrackOpticalSource {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!(
            "Starting DTrack optical source '{}' on port {}",
            self.base.name(),
            self.m_port
        );

        let port = self.m_port;
        let multicast_group = self.m_multicast_group.clone();
        let timeout = self.m_timeout;
        let body_ids = self.m_body_ids.clone();
        let world_trafo = self.m_world_trafo;
        let body_trafo = self.m_body_trafo;
        let ref_quat = self.m_ref_to_optical_quat;
        let ref_vec = self.m_ref_to_optical_vec;
        let node_name = self.base.name().to_string();
        let consumers = self.base.consumers_arc();
        let enabled = self.base.enabled_arc();
        let done = self.m_done.clone();

        done.store(false, Ordering::Relaxed);

        self.m_worker_handle = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                // Bind UDP socket.
                let bind_addr = format!("0.0.0.0:{}", port);
                let socket = match UdpSocket::bind(&bind_addr) {
                    Ok(s) => s,
                    Err(e) => {
                        log::error!(
                            "DTrack '{}': could not bind to {}: {}",
                            node_name,
                            bind_addr,
                            e
                        );
                        return;
                    }
                };

                if let Err(e) = socket.set_read_timeout(Some(timeout)) {
                    log::warn!("DTrack '{}': could not set read timeout: {}", node_name, e);
                }

                // Join multicast group if configured.
                if !multicast_group.is_empty() {
                    if let Ok(mcast_addr) = multicast_group.parse::<Ipv4Addr>() {
                        // Use socket2 for multicast join.
                        use socket2::Socket;
                        #[cfg(unix)]
                        use std::os::unix::io::{AsRawFd, FromRawFd};
                        #[cfg(windows)]
                        use std::os::windows::io::{AsRawSocket, FromRawSocket};

                        #[cfg(unix)]
                        let s2 = unsafe {
                            Socket::from_raw_fd(socket.as_raw_fd())
                        };
                        #[cfg(windows)]
                        let s2 = unsafe {
                            Socket::from_raw_socket(socket.as_raw_socket())
                        };

                        match s2.join_multicast_v4(&mcast_addr, &Ipv4Addr::UNSPECIFIED) {
                            Ok(_) => {
                                log::info!(
                                    "DTrack '{}': joined multicast group {}",
                                    node_name,
                                    multicast_group
                                );
                            }
                            Err(e) => {
                                log::warn!(
                                    "DTrack '{}': failed to join multicast group {}: {}",
                                    node_name,
                                    multicast_group,
                                    e
                                );
                            }
                        }

                        // Prevent socket2 from closing the fd/socket -- ownership stays with UdpSocket.
                        #[cfg(unix)]
                        {
                            use std::os::unix::io::IntoRawFd;
                            let _ = s2.into_raw_fd();
                        }
                        #[cfg(windows)]
                        {
                            use std::os::windows::io::IntoRawSocket;
                            let _ = s2.into_raw_socket();
                        }
                    } else {
                        log::warn!(
                            "DTrack '{}': not a valid multicast group '{}'",
                            node_name,
                            multicast_group
                        );
                    }
                }

                log::info!("DTrack '{}': listening on {}", node_name, bind_addr);

                // State for the receive loop.
                // previousTimestamp initialized to > 86400 so any first timestamp is smaller.
                let mut previous_timestamp: f64 = 100000.0;
                let mut midnight = compute_midnight_utc(SystemTime::now());
                let mut first_pass = true;
                let mut body_last_timestamp: HashMap<i32, Instant> = HashMap::new();
                let dt_limit: f64 = 0.001; // Block unrealistically small timestamp increments.

                // Fallback / min / max latency constants (same as C++ defaults).
                let min_latency = Duration::from_micros(0);
                let max_latency = Duration::from_micros(50_000);
                let fallback_latency = Duration::from_micros(10_000);

                let mut buf = [0u8; 65536];

                while !done.load(Ordering::Relaxed) {
                    // Receive a UDP packet.
                    let n = match socket.recv(&mut buf) {
                        Ok(n) => n,
                        Err(ref e)
                            if e.kind() == std::io::ErrorKind::TimedOut
                                || e.kind() == std::io::ErrorKind::WouldBlock =>
                        {
                            continue;
                        }
                        Err(e) => {
                            log::warn!("DTrack '{}': recv error: {}", node_name, e);
                            continue;
                        }
                    };

                    let packet = match std::str::from_utf8(&buf[..n]) {
                        Ok(s) => s,
                        Err(e) => {
                            log::warn!("DTrack '{}': invalid UTF-8 in packet: {}", node_name, e);
                            continue;
                        }
                    };

                    let frame = parse_dtrack_packet(packet);

                    let timestamp = match frame.timestamp {
                        Some(ts) => ts,
                        None => continue,
                    };

                    let time_now = SystemTime::now();
                    let instant_now = Instant::now();

                    // Detect midnight rollover: if new timestamp is more than 10s
                    // behind previous, we crossed midnight.
                    if timestamp + 10.0 < previous_timestamp {
                        log::info!(
                            "DTrack '{}': updating midnight timestamp: {} previousTimestamp {}",
                            node_name,
                            timestamp,
                            previous_timestamp
                        );
                        midnight = compute_midnight_utc(time_now);
                    }

                    // Process each body.
                    for body in &frame.bodies {
                        // Filter by configured body IDs.
                        if !body_ids.contains(&body.id) {
                            continue;
                        }

                        // Detect midnight rollover per-body (same check as C++).
                        if timestamp + 10.0 < previous_timestamp {
                            previous_timestamp = timestamp;
                            // Skip this body on midnight rollover (as C++ does).
                            continue;
                        }

                        let delta_time =
                            Duration::from_secs_f64((timestamp - previous_timestamp).max(0.0));
                        previous_timestamp = timestamp;

                        // Compute latency.
                        let time_measured =
                            midnight + Duration::from_secs_f64(timestamp);
                        let apparent_latency = time_now
                            .duration_since(time_measured)
                            .unwrap_or(Duration::ZERO);
                        let is_probably_synced =
                            apparent_latency > min_latency && apparent_latency < max_latency;

                        let latency = if is_probably_synced {
                            // Clamp apparent latency to [min, max].
                            let clamped = apparent_latency.max(min_latency).min(max_latency);
                            clamped
                        } else {
                            fallback_latency
                        };

                        // Convert position (mm -> m).
                        let position =
                            Self::convert_location(&world_trafo, &body.loc);

                        // Skip bodies with zero position (not tracked).
                        if position == Vec3d::zeros() {
                            continue;
                        }

                        // Convert rotation.
                        let orientation = Self::convert_rotation(
                            &world_trafo,
                            &body_trafo,
                            &body.rot,
                        );

                        // Apply reference-to-optical-frame correction.
                        let (position, orientation) = Self::correct_pose(
                            position,
                            orientation,
                            &ref_quat,
                            &ref_vec,
                        );

                        // Block unrealistically small timestamp increments.
                        let body_last = body_last_timestamp.get(&body.id).copied();
                        if let Some(last) = body_last {
                            let dt = instant_now.duration_since(last).as_secs_f64();
                            if dt < dt_limit && !first_pass {
                                log::debug!(
                                    "DTrack '{}': blocked unrealistically small timestamp increment for body {}",
                                    node_name,
                                    body.id
                                );
                                continue;
                            }
                        }

                        first_pass = false;
                        body_last_timestamp.insert(body.id, instant_now);

                        // Build OpticalData.
                        let pose = OpticalData {
                            sender_id: (body.id + 1).to_string(), // Convert back to 1-based.
                            timestamp: time_now,
                            last_data_time: time_now,
                            latency: latency.as_secs_f64(),
                            position,
                            orientation,
                            angular_velocity: Vec3d::zeros(),
                            quality: body.quality,
                            frame_rate: 0.0,
                            frame_number: frame.frame_number.unwrap_or(0) as i32,
                            interval: delta_time,
                        };

                        // Emit to consumers.
                        if enabled.load(Ordering::Relaxed) {
                            let cbs = consumers.lock().unwrap();
                            let data = StreamableData::Optical(pose);
                            for cb in cbs.iter() {
                                cb(data.clone());
                            }
                        }
                    }
                }

                log::info!("DTrack '{}': worker thread exiting", node_name);
            })
            .await;

            if let Err(e) = result {
                log::warn!("DTrack worker thread panicked: {}", e);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping DTrack optical source: {}", self.base.name());
        self.m_done.store(true, Ordering::Relaxed);

        if let Some(handle) = self.m_worker_handle.take() {
            handle.abort();
        }

        self.base.stop_heartbeat();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }

    fn set_on_output(&self, callback: ConsumerCallback) {
        self.base.add_consumer(callback);
    }

    fn receive_data(&mut self, _data: StreamableData) {
        // Source node - does not receive data from upstream.
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_parse_floats() {
        let vals = parse_floats("123.456 789.012 345.678");
        assert_eq!(vals.len(), 3);
        assert_relative_eq!(vals[0], 123.456, epsilon = 1e-6);
        assert_relative_eq!(vals[1], 789.012, epsilon = 1e-6);
        assert_relative_eq!(vals[2], 345.678, epsilon = 1e-6);
    }

    #[test]
    fn test_extract_bracket_groups() {
        let groups = extract_bracket_groups("[1 0.5][10 20 30][0.1 0.2 0.3 0.4 0.5 0.6 0.7 0.8 0.9]");
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0], "1 0.5");
        assert_eq!(groups[1], "10 20 30");
        assert_eq!(groups[2], "0.1 0.2 0.3 0.4 0.5 0.6 0.7 0.8 0.9");
    }

    #[test]
    fn test_parse_6d_line_single_body() {
        let line = "1 [0 1.000][123.456 789.012 345.678][0.1 0.2 0.3 0.4 0.5 0.6 0.7 0.8 0.9]";
        let bodies = parse_6d_line(line);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].id, 0);
        assert_relative_eq!(bodies[0].quality, 1.0, epsilon = 1e-6);
        assert_relative_eq!(bodies[0].loc[0], 123.456, epsilon = 1e-6);
        assert_relative_eq!(bodies[0].loc[1], 789.012, epsilon = 1e-6);
        assert_relative_eq!(bodies[0].loc[2], 345.678, epsilon = 1e-6);
    }

    #[test]
    fn test_parse_6d_line_two_bodies() {
        let line = "2 [0 1.000][123.456 789.012 345.678][0.1 0.2 0.3 0.4 0.5 0.6 0.7 0.8 0.9] [1 0.950][111.111 222.222 333.333][0.9 0.8 0.7 0.6 0.5 0.4 0.3 0.2 0.1]";
        let bodies = parse_6d_line(line);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].id, 0);
        assert_eq!(bodies[1].id, 1);
        assert_relative_eq!(bodies[1].quality, 0.950, epsilon = 1e-6);
        assert_relative_eq!(bodies[1].loc[0], 111.111, epsilon = 1e-6);
    }

    #[test]
    fn test_parse_dtrack_packet() {
        let packet = "fr 12345\nts 45678.123456\n6d 1 [0 1.000][1000.0 2000.0 3000.0][1 0 0 0 1 0 0 0 1]\n";
        let frame = parse_dtrack_packet(packet);
        assert_eq!(frame.frame_number, Some(12345));
        assert_relative_eq!(frame.timestamp.unwrap(), 45678.123456, epsilon = 1e-6);
        assert_eq!(frame.bodies.len(), 1);
        assert_eq!(frame.bodies[0].id, 0);
    }

    #[test]
    fn test_rotation_for_directions_identity() {
        // SteamVR convention: right, up, backward = identity.
        let dirs = [
            "right".to_string(),
            "up".to_string(),
            "backward".to_string(),
        ];
        let rot = rotation_for_directions(&dirs).unwrap();
        assert_relative_eq!(rot, Matrix3::identity(), epsilon = 1e-10);
    }

    #[test]
    fn test_rotation_for_directions_flipped() {
        // left, down, forward should give -I (still orthogonal but det = -1).
        let dirs = [
            "left".to_string(),
            "down".to_string(),
            "forward".to_string(),
        ];
        let rot = rotation_for_directions(&dirs).unwrap();
        assert_relative_eq!(rot, -Matrix3::identity(), epsilon = 1e-10);
    }

    #[test]
    fn test_rotation_for_directions_invalid() {
        let dirs = [
            "right".to_string(),
            "right".to_string(),
            "right".to_string(),
        ];
        assert!(rotation_for_directions(&dirs).is_err());
    }

    #[test]
    fn test_convert_location_identity() {
        let world_trafo = Isometry3::<f64>::identity();
        let loc = [1000.0, 2000.0, 3000.0];
        let result = DTrackOpticalSource::convert_location(&world_trafo, &loc);
        assert_relative_eq!(result.x, 1.0, epsilon = 1e-10);
        assert_relative_eq!(result.y, 2.0, epsilon = 1e-10);
        assert_relative_eq!(result.z, 3.0, epsilon = 1e-10);
    }

    #[test]
    fn test_convert_rotation_identity() {
        let world_trafo = Isometry3::<f64>::identity();
        let body_trafo = Matrix3::identity();
        // Identity rotation in column-major: [1 0 0 0 1 0 0 0 1].
        let rot = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let result = DTrackOpticalSource::convert_rotation(&world_trafo, &body_trafo, &rot);
        assert_relative_eq!(
            result.angle(),
            0.0,
            epsilon = 1e-10
        );
    }

    #[test]
    fn test_correct_pose_identity() {
        let position = Vector3::new(1.0, 2.0, 3.0);
        let orientation = UnitQuaternion::identity();
        let ref_quat = UnitQuaternion::identity();
        let ref_vec = Vector3::zeros();

        let (pos, ori) = DTrackOpticalSource::correct_pose(
            position, orientation, &ref_quat, &ref_vec,
        );
        assert_relative_eq!(pos, position, epsilon = 1e-10);
        assert_relative_eq!(ori.angle(), 0.0, epsilon = 1e-10);
    }

    #[test]
    fn test_correct_pose_with_offset() {
        let position = Vector3::new(1.0, 0.0, 0.0);
        let orientation = UnitQuaternion::identity();
        let ref_quat = UnitQuaternion::identity();
        let ref_vec = Vector3::new(0.0, 0.5, 0.0);

        let (pos, _ori) = DTrackOpticalSource::correct_pose(
            position, orientation, &ref_quat, &ref_vec,
        );
        assert_relative_eq!(pos.x, 1.0, epsilon = 1e-10);
        assert_relative_eq!(pos.y, 0.5, epsilon = 1e-10);
        assert_relative_eq!(pos.z, 0.0, epsilon = 1e-10);
    }

    #[test]
    fn test_body_id_parsing() {
        let config = serde_json::json!({
            "bodyIDs": [1, 3, 5],
            "port": 5000
        });
        let source = DTrackOpticalSource::new("test", &config);
        // 1-based -> 0-based: [0, 2, 4]
        assert_eq!(source.m_body_ids, vec![0, 2, 4]);
    }

    #[test]
    fn test_vec_for_direction() {
        assert_eq!(vec_for_direction("right"), Some(Vector3::x()));
        assert_eq!(vec_for_direction("left"), Some(-Vector3::x()));
        assert_eq!(vec_for_direction("up"), Some(Vector3::y()));
        assert_eq!(vec_for_direction("down"), Some(-Vector3::y()));
        assert_eq!(vec_for_direction("forward"), Some(-Vector3::z()));
        assert_eq!(vec_for_direction("backward"), Some(Vector3::z()));
        assert_eq!(vec_for_direction("invalid"), None);
    }

    #[test]
    fn test_compute_midnight_utc() {
        let now = SystemTime::now();
        let midnight = compute_midnight_utc(now);
        let since_midnight = now.duration_since(midnight).unwrap();
        // Should be less than 24 hours.
        assert!(since_midnight.as_secs() < 86400);
    }

    #[test]
    fn test_parse_quat_array() {
        let config = serde_json::json!({
            "referenceToOpticalFrameQuat": [1, 0, 0, 0]
        });
        let q = parse_quat(&config, "referenceToOpticalFrameQuat");
        assert_relative_eq!(q.w, 1.0, epsilon = 1e-10);
        assert_relative_eq!(q.i, 0.0, epsilon = 1e-10);
        assert_relative_eq!(q.j, 0.0, epsilon = 1e-10);
        assert_relative_eq!(q.k, 0.0, epsilon = 1e-10);
    }

    #[test]
    fn test_parse_vec3_array() {
        let config = serde_json::json!({
            "referenceToOpticalFrameVec": [1.0, 2.0, 3.0]
        });
        let v = parse_vec3(&config, "referenceToOpticalFrameVec");
        assert_relative_eq!(v.x, 1.0, epsilon = 1e-10);
        assert_relative_eq!(v.y, 2.0, epsilon = 1e-10);
        assert_relative_eq!(v.z, 3.0, epsilon = 1e-10);
    }

    #[test]
    fn test_default_config() {
        let config = serde_json::json!({
            "bodyIDs": [1],
            "port": 5000
        });
        let source = DTrackOpticalSource::new("test", &config);
        assert_eq!(source.m_port, 5000);
        assert_eq!(source.m_body_ids, vec![0]);
        assert!(source.m_multicast_group.is_empty());
        assert_eq!(source.m_timeout, Duration::from_millis(100));
    }

    #[test]
    fn test_column_major_rotation() {
        // 90-degree rotation about Z: in column-major the columns are the
        // basis vectors of the rotated frame expressed in room coordinates.
        // X-axis -> (0, 1, 0), Y-axis -> (-1, 0, 0), Z-axis -> (0, 0, 1)
        // Column-major storage: col0 = [0, 1, 0], col1 = [-1, 0, 0], col2 = [0, 0, 1]
        let rot = [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let world_trafo = Isometry3::<f64>::identity();
        let body_trafo = Matrix3::identity();
        let q = DTrackOpticalSource::convert_rotation(&world_trafo, &body_trafo, &rot);

        // Expected: 90-degree rotation about Z.
        let expected =
            UnitQuaternion::from_axis_angle(&Vector3::z_axis(), std::f64::consts::FRAC_PI_2);
        assert_relative_eq!(
            q.angle_to(&expected),
            0.0,
            epsilon = 1e-10
        );
    }
}
