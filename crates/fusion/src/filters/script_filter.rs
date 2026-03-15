use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fusion_registry::{sf, SettingsField};
use fusion_types::{
    ApiRequest, FusedPose, GnssData, ImuData, JsonValueExt, OpticalData, StreamableData,
    VehicleSpeed, VelocityMeterData, Vec3d, Quatd,
};
use nalgebra::Quaternion;
use rhai::{Dynamic, Engine, Map, Scope, AST};
use serde_json::json;

use crate::node::{CommandConsumerCallback, ConsumerCallback, Node};

const DEFAULT_SCRIPT: &str = "fn process(data) {\n    data\n}";

pub fn settings_schema() -> Vec<SettingsField> {
    vec![
        sf("script", "Script", "code", json!(DEFAULT_SCRIPT)),
        sf("inputCount", "Input Ports", "number", json!(1)),
        sf("outputCount", "Output Ports", "number", json!(1)),
    ]
}

// ---------------------------------------------------------------------------
// Type conversion helpers
// ---------------------------------------------------------------------------

fn get_float(m: &Map, key: &str, default: f64) -> f64 {
    m.get(key)
        .and_then(|v| v.as_float().ok().or_else(|| v.as_int().ok().map(|i| i as f64)))
        .unwrap_or(default)
}

fn get_int(m: &Map, key: &str, default: i64) -> i64 {
    m.get(key)
        .and_then(|v| v.as_int().ok().or_else(|| v.as_float().ok().map(|f| f as i64)))
        .unwrap_or(default)
}

fn get_bool(m: &Map, key: &str, default: bool) -> bool {
    m.get(key).and_then(|v| v.as_bool().ok()).unwrap_or(default)
}

fn get_string(m: &Map, key: &str) -> String {
    m.get(key)
        .and_then(|v| v.clone().into_string().ok())
        .unwrap_or_default()
}

fn get_map(m: &Map, key: &str) -> Map {
    m.get(key)
        .and_then(|v| v.clone().try_cast::<Map>())
        .unwrap_or_default()
}

fn timepoint_to_nanos(tp: &SystemTime) -> i64 {
    tp.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as i64
}

fn nanos_to_timepoint(nanos: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(nanos.max(0) as u64)
}

fn vec3_to_map(v: &Vec3d) -> Map {
    let mut m = Map::new();
    m.insert("x".into(), Dynamic::from_float(v.x));
    m.insert("y".into(), Dynamic::from_float(v.y));
    m.insert("z".into(), Dynamic::from_float(v.z));
    m
}

fn map_to_vec3(m: &Map) -> Vec3d {
    Vec3d::new(get_float(m, "x", 0.0), get_float(m, "y", 0.0), get_float(m, "z", 0.0))
}

fn quat_to_map(q: &Quatd) -> Map {
    let mut m = Map::new();
    m.insert("w".into(), Dynamic::from_float(q.w));
    m.insert("x".into(), Dynamic::from_float(q.i));
    m.insert("y".into(), Dynamic::from_float(q.j));
    m.insert("z".into(), Dynamic::from_float(q.k));
    m
}

fn map_to_quat(m: &Map) -> Quatd {
    Quatd::new_normalize(Quaternion::new(
        get_float(m, "w", 1.0),
        get_float(m, "x", 0.0),
        get_float(m, "y", 0.0),
        get_float(m, "z", 0.0),
    ))
}

// ---------------------------------------------------------------------------
// StreamableData ↔ Rhai Map conversions
// ---------------------------------------------------------------------------

fn imu_to_map(d: &ImuData) -> Map {
    let mut m = Map::new();
    m.insert("_type".into(), Dynamic::from("Imu".to_string()));
    m.insert("sender_id".into(), Dynamic::from(d.sender_id.clone()));
    m.insert("timestamp".into(), Dynamic::from_int(timepoint_to_nanos(&d.timestamp)));
    m.insert("gyroscope".into(), Dynamic::from(vec3_to_map(&d.gyroscope)));
    m.insert("accelerometer".into(), Dynamic::from(vec3_to_map(&d.accelerometer)));
    m.insert("quaternion".into(), Dynamic::from(quat_to_map(&d.quaternion)));
    m.insert("euler".into(), Dynamic::from(vec3_to_map(&d.euler)));
    m.insert("period".into(), Dynamic::from_float(d.period));
    m.insert("linear_velocity".into(), Dynamic::from(vec3_to_map(&d.linear_velocity)));
    m
}

fn map_to_imu(m: &Map) -> ImuData {
    ImuData {
        sender_id: get_string(m, "sender_id"),
        timestamp: nanos_to_timepoint(get_int(m, "timestamp", 0)),
        gyroscope: map_to_vec3(&get_map(m, "gyroscope")),
        accelerometer: map_to_vec3(&get_map(m, "accelerometer")),
        quaternion: map_to_quat(&get_map(m, "quaternion")),
        euler: map_to_vec3(&get_map(m, "euler")),
        period: get_float(m, "period", 0.0),
        linear_velocity: map_to_vec3(&get_map(m, "linear_velocity")),
        ..Default::default()
    }
}

fn gnss_to_map(d: &GnssData) -> Map {
    let mut m = Map::new();
    m.insert("_type".into(), Dynamic::from("Gnss".to_string()));
    m.insert("sender_id".into(), Dynamic::from(d.sender_id.clone()));
    m.insert("timestamp".into(), Dynamic::from_int(timepoint_to_nanos(&d.timestamp)));
    m.insert("latitude".into(), Dynamic::from_float(d.latitude));
    m.insert("longitude".into(), Dynamic::from_float(d.longitude));
    m.insert("altitude".into(), Dynamic::from_float(d.altitude));
    m.insert("undulation".into(), Dynamic::from_float(d.undulation));
    m.insert("height".into(), Dynamic::from_float(d.height));
    m.insert("quality".into(), Dynamic::from_int(d.quality as i64));
    m.insert("n_sat".into(), Dynamic::from_int(d.n_sat as i64));
    m.insert("hdop".into(), Dynamic::from_float(d.hdop));
    m.insert("tmg".into(), Dynamic::from_float(d.tmg));
    m.insert("heading".into(), Dynamic::from_float(d.heading));
    m.insert("orientation".into(), Dynamic::from(quat_to_map(&d.orientation)));
    m.insert("diff_age".into(), Dynamic::from_float(d.diff_age));
    m
}

fn map_to_gnss(m: &Map) -> GnssData {
    GnssData {
        sender_id: get_string(m, "sender_id"),
        timestamp: nanos_to_timepoint(get_int(m, "timestamp", 0)),
        latitude: get_float(m, "latitude", 0.0),
        longitude: get_float(m, "longitude", 0.0),
        altitude: get_float(m, "altitude", 0.0),
        undulation: get_float(m, "undulation", 0.0),
        height: get_float(m, "height", 0.0),
        quality: get_int(m, "quality", 0) as i32,
        n_sat: get_int(m, "n_sat", 0) as i32,
        hdop: get_float(m, "hdop", 0.0),
        tmg: get_float(m, "tmg", 0.0),
        heading: get_float(m, "heading", 0.0),
        orientation: map_to_quat(&get_map(m, "orientation")),
        diff_age: get_float(m, "diff_age", 0.0),
        ..Default::default()
    }
}

fn optical_to_map(d: &OpticalData) -> Map {
    let mut m = Map::new();
    m.insert("_type".into(), Dynamic::from("Optical".to_string()));
    m.insert("sender_id".into(), Dynamic::from(d.sender_id.clone()));
    m.insert("timestamp".into(), Dynamic::from_int(timepoint_to_nanos(&d.timestamp)));
    m.insert("position".into(), Dynamic::from(vec3_to_map(&d.position)));
    m.insert("orientation".into(), Dynamic::from(quat_to_map(&d.orientation)));
    m.insert("angular_velocity".into(), Dynamic::from(vec3_to_map(&d.angular_velocity)));
    m
}

fn map_to_optical(m: &Map) -> OpticalData {
    OpticalData {
        sender_id: get_string(m, "sender_id"),
        timestamp: nanos_to_timepoint(get_int(m, "timestamp", 0)),
        position: map_to_vec3(&get_map(m, "position")),
        orientation: map_to_quat(&get_map(m, "orientation")),
        angular_velocity: map_to_vec3(&get_map(m, "angular_velocity")),
        ..Default::default()
    }
}

fn fused_pose_to_map(d: &FusedPose) -> Map {
    let mut m = Map::new();
    m.insert("_type".into(), Dynamic::from("FusedPose".to_string()));
    m.insert("sender_id".into(), Dynamic::from(d.sender_id.clone()));
    m.insert("timestamp".into(), Dynamic::from_int(timepoint_to_nanos(&d.timestamp)));
    m.insert("transmission_time".into(), Dynamic::from_int(timepoint_to_nanos(&d.transmission_time)));
    m.insert("last_data_time".into(), Dynamic::from_int(timepoint_to_nanos(&d.last_data_time)));
    m.insert("position".into(), Dynamic::from(vec3_to_map(&d.position)));
    m.insert("orientation".into(), Dynamic::from(quat_to_map(&d.orientation)));
    m.insert("angular_velocity".into(), Dynamic::from(vec3_to_map(&d.angular_velocity)));
    m.insert("velocity".into(), Dynamic::from(vec3_to_map(&d.velocity)));
    m.insert("acceleration".into(), Dynamic::from(vec3_to_map(&d.acceleration)));
    m.insert("frame_number".into(), Dynamic::from_int(d.frame_number));
    m
}

fn map_to_fused_pose(m: &Map) -> FusedPose {
    FusedPose {
        sender_id: get_string(m, "sender_id"),
        timestamp: nanos_to_timepoint(get_int(m, "timestamp", 0)),
        transmission_time: nanos_to_timepoint(get_int(m, "transmission_time", 0)),
        last_data_time: nanos_to_timepoint(get_int(m, "last_data_time", 0)),
        position: map_to_vec3(&get_map(m, "position")),
        orientation: map_to_quat(&get_map(m, "orientation")),
        angular_velocity: map_to_vec3(&get_map(m, "angular_velocity")),
        velocity: map_to_vec3(&get_map(m, "velocity")),
        acceleration: map_to_vec3(&get_map(m, "acceleration")),
        frame_number: get_int(m, "frame_number", 0),
        ..Default::default()
    }
}

fn vehicle_speed_to_map(d: &VehicleSpeed) -> Map {
    let mut m = Map::new();
    m.insert("_type".into(), Dynamic::from("VehicleSpeed".to_string()));
    m.insert("sender_id".into(), Dynamic::from(d.sender_id.clone()));
    m.insert("timestamp".into(), Dynamic::from_int(timepoint_to_nanos(&d.timestamp)));
    m.insert("linear".into(), Dynamic::from_float(d.linear));
    m.insert("angular".into(), Dynamic::from_float(d.angular));
    m.insert("valid_angular".into(), Dynamic::from_bool(d.valid_angular));
    m
}

fn map_to_vehicle_speed(m: &Map) -> VehicleSpeed {
    VehicleSpeed {
        sender_id: get_string(m, "sender_id"),
        timestamp: nanos_to_timepoint(get_int(m, "timestamp", 0)),
        linear: get_float(m, "linear", 0.0),
        angular: get_float(m, "angular", 0.0),
        valid_angular: get_bool(m, "valid_angular", false),
    }
}

fn velocity_meter_to_map(d: &VelocityMeterData) -> Map {
    let mut m = Map::new();
    m.insert("_type".into(), Dynamic::from("VelocityMeter".to_string()));
    m.insert("sender_id".into(), Dynamic::from(d.sender_id.clone()));
    m.insert("timestamp".into(), Dynamic::from_int(timepoint_to_nanos(&d.timestamp)));
    m.insert("counter".into(), Dynamic::from_int(d.counter as i64));
    m.insert("velocity".into(), Dynamic::from_float(d.velocity));
    m.insert("distance".into(), Dynamic::from_float(d.distance));
    m.insert("material".into(), Dynamic::from_float(d.material));
    m.insert("doppler_level".into(), Dynamic::from_float(d.doppler_level));
    m.insert("output_status".into(), Dynamic::from_int(d.output_status as i64));
    m
}

fn map_to_velocity_meter(m: &Map) -> VelocityMeterData {
    VelocityMeterData {
        sender_id: get_string(m, "sender_id"),
        timestamp: nanos_to_timepoint(get_int(m, "timestamp", 0)),
        counter: get_int(m, "counter", 0) as i32,
        velocity: get_float(m, "velocity", 0.0),
        distance: get_float(m, "distance", 0.0),
        material: get_float(m, "material", 0.0),
        doppler_level: get_float(m, "doppler_level", 0.0),
        output_status: get_int(m, "output_status", 0) as i32,
    }
}

fn streamable_to_map(data: &StreamableData) -> Option<(String, Map)> {
    match data {
        StreamableData::Imu(d) => Some(("Imu".into(), imu_to_map(d))),
        StreamableData::Gnss(d) => Some(("Gnss".into(), gnss_to_map(d))),
        StreamableData::Optical(d) => Some(("Optical".into(), optical_to_map(d))),
        StreamableData::FusedPose(d) => Some(("FusedPose".into(), fused_pose_to_map(d))),
        StreamableData::VehicleSpeed(d) => Some(("VehicleSpeed".into(), vehicle_speed_to_map(d))),
        StreamableData::VelocityMeter(d) => Some(("VelocityMeter".into(), velocity_meter_to_map(d))),
        _ => None,
    }
}

fn map_to_streamable(type_name: &str, m: &Map) -> Option<StreamableData> {
    match type_name {
        "Imu" => Some(StreamableData::Imu(map_to_imu(m))),
        "Gnss" => Some(StreamableData::Gnss(map_to_gnss(m))),
        "Optical" => Some(StreamableData::Optical(map_to_optical(m))),
        "FusedPose" => Some(StreamableData::FusedPose(map_to_fused_pose(m))),
        "VehicleSpeed" => Some(StreamableData::VehicleSpeed(map_to_vehicle_speed(m))),
        "VelocityMeter" => Some(StreamableData::VelocityMeter(map_to_velocity_meter(m))),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ScriptFilter
// ---------------------------------------------------------------------------

pub struct ScriptFilter {
    m_name: String,
    m_enabled: bool,
    m_engine: Engine,
    m_ast: Option<AST>,
    m_script_source: String,
    m_last_error: Option<String>,
    m_frame_count: u64,
    m_last_exec_us: u64,
    m_state: Map,
    m_on_output: Arc<Mutex<Option<ConsumerCallback>>>,
    m_on_command_output: Arc<Mutex<Option<CommandConsumerCallback>>>,
}

fn create_engine() -> Engine {
    let mut engine = Engine::new();

    // Prevent infinite loops
    engine.set_max_operations(1_000_000);

    // Math helpers
    engine.register_fn("deg2rad", |deg: f64| -> f64 {
        deg * std::f64::consts::PI / 180.0
    });
    engine.register_fn("rad2deg", |rad: f64| -> f64 {
        rad * 180.0 / std::f64::consts::PI
    });

    // Vector helpers
    engine.register_fn("vec3", |x: f64, y: f64, z: f64| -> Map {
        vec3_to_map(&Vec3d::new(x, y, z))
    });
    engine.register_fn("vec3_length", |v: Map| -> f64 {
        let x = get_float(&v, "x", 0.0);
        let y = get_float(&v, "y", 0.0);
        let z = get_float(&v, "z", 0.0);
        (x * x + y * y + z * z).sqrt()
    });
    engine.register_fn("vec3_scale", |v: Map, s: f64| -> Map {
        let mut out = Map::new();
        out.insert("x".into(), Dynamic::from_float(get_float(&v, "x", 0.0) * s));
        out.insert("y".into(), Dynamic::from_float(get_float(&v, "y", 0.0) * s));
        out.insert("z".into(), Dynamic::from_float(get_float(&v, "z", 0.0) * s));
        out
    });
    engine.register_fn("vec3_add", |a: Map, b: Map| -> Map {
        let mut out = Map::new();
        out.insert("x".into(), Dynamic::from_float(get_float(&a, "x", 0.0) + get_float(&b, "x", 0.0)));
        out.insert("y".into(), Dynamic::from_float(get_float(&a, "y", 0.0) + get_float(&b, "y", 0.0)));
        out.insert("z".into(), Dynamic::from_float(get_float(&a, "z", 0.0) + get_float(&b, "z", 0.0)));
        out
    });
    engine.register_fn("vec3_dot", |a: Map, b: Map| -> f64 {
        get_float(&a, "x", 0.0) * get_float(&b, "x", 0.0)
            + get_float(&a, "y", 0.0) * get_float(&b, "y", 0.0)
            + get_float(&a, "z", 0.0) * get_float(&b, "z", 0.0)
    });

    // Quaternion helper
    engine.register_fn("quat", |w: f64, x: f64, y: f64, z: f64| -> Map {
        quat_to_map(&Quatd::new_normalize(Quaternion::new(w, x, y, z)))
    });

    // Logging
    engine.register_fn("log_info", |msg: &str| log::info!("[script] {}", msg));
    engine.register_fn("log_warn", |msg: &str| log::warn!("[script] {}", msg));
    engine.register_fn("log_debug", |msg: &str| log::debug!("[script] {}", msg));

    engine
}

impl ScriptFilter {
    pub fn new(name: &str, settings: serde_json::Value) -> Self {
        let script = settings.value_str("script", DEFAULT_SCRIPT);
        let engine = create_engine();

        let (ast, error) = match engine.compile(&script) {
            Ok(ast) => (Some(ast), None),
            Err(e) => {
                let msg = format!("{}", e);
                log::error!("[{}] Script compile error: {}", name, msg);
                (None, Some(msg))
            }
        };

        Self {
            m_name: name.to_owned(),
            m_enabled: true,
            m_engine: engine,
            m_ast: ast,
            m_script_source: script,
            m_last_error: error,
            m_frame_count: 0,
            m_last_exec_us: 0,
            m_state: Map::new(),
            m_on_output: Arc::new(Mutex::new(None)),
            m_on_command_output: Arc::new(Mutex::new(None)),
        }
    }

    fn process_data(&mut self, data: StreamableData) {
        let ast = match &self.m_ast {
            Some(ast) => ast,
            None => return,
        };

        // Pass through unmapped data types unchanged
        let (type_name, data_map) = match streamable_to_map(&data) {
            Some(v) => v,
            None => {
                self.emit(data);
                return;
            }
        };

        let start = Instant::now();
        let mut scope = Scope::new();
        scope.push("state", self.m_state.clone());

        let result: Result<Dynamic, _> =
            self.m_engine.call_fn(&mut scope, ast, "process", (Dynamic::from(data_map),));

        // Retrieve potentially mutated state
        if let Some(new_state) = scope.remove::<Map>("state") {
            self.m_state = new_state;
        }

        self.m_last_exec_us = start.elapsed().as_micros() as u64;
        self.m_frame_count += 1;

        match result {
            Ok(value) => {
                self.m_last_error = None;
                // Returning () drops the message
                if value.is_unit() {
                    return;
                }
                if let Some(out_map) = value.try_cast::<Map>() {
                    let out_type = out_map
                        .get("_type")
                        .and_then(|v| v.clone().into_string().ok())
                        .unwrap_or_else(|| type_name.clone());
                    if let Some(out_data) = map_to_streamable(&out_type, &out_map) {
                        self.emit(out_data);
                    } else {
                        log::warn!("[{}] Unknown output type '{}'", self.m_name, out_type);
                    }
                }
            }
            Err(e) => {
                let msg = format!("{}", e);
                if self.m_last_error.as_deref() != Some(&msg) {
                    log::error!("[{}] Script runtime error: {}", self.m_name, msg);
                }
                self.m_last_error = Some(msg);
            }
        }
    }

    fn update_script(&mut self, script: &str) {
        match self.m_engine.compile(script) {
            Ok(ast) => {
                self.m_ast = Some(ast);
                self.m_script_source = script.to_owned();
                self.m_last_error = None;
                self.m_state.clear();
                log::info!("[{}] Script updated", self.m_name);
            }
            Err(e) => {
                let msg = format!("Compile error: {}", e);
                log::error!("[{}] {}", self.m_name, msg);
                self.m_last_error = Some(msg);
            }
        }
    }

    fn process_command(&mut self, cmd: &ApiRequest) {
        if !cmd.topic.contains(&self.m_name) {
            return;
        }

        if cmd.command == "setConfigJsonPath" {
            if let Some(script_val) = cmd
                .data
                .get("script")
                .or_else(|| cmd.data.get("settings").and_then(|s| s.get("script")))
            {
                if let Some(new_script) = script_val.as_str() {
                    self.update_script(new_script);
                }
            }

            let return_data = json!({
                "script": self.m_script_source,
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

impl Node for ScriptFilter {
    fn name(&self) -> &str {
        &self.m_name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.m_frame_count = 0;
        self.m_state.clear();
        log::info!("[{}] ScriptFilter started", self.m_name);
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!(
            "[{}] ScriptFilter stopped ({} frames)",
            self.m_name,
            self.m_frame_count
        );
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.m_enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.m_enabled = enabled;
    }

    fn receive_data(&mut self, data: StreamableData) {
        if !self.m_enabled {
            return;
        }
        self.process_data(data);
    }

    fn receive_command(&mut self, cmd: &ApiRequest) {
        self.process_command(cmd);
    }

    fn set_on_output(&self, callback: ConsumerCallback) {
        *self.m_on_output.lock().unwrap() = Some(callback);
    }

    fn set_on_command_output(&self, callback: CommandConsumerCallback) {
        *self.m_on_command_output.lock().unwrap() = Some(callback);
    }

    fn status(&self) -> serde_json::Value {
        json!({
            "frameCount": self.m_frame_count,
            "lastExecUs": self.m_last_exec_us,
            "scriptCompiled": self.m_ast.is_some(),
            "lastError": self.m_last_error,
            "stateKeys": self.m_state.keys().map(|k| k.to_string()).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_imu(sender: &str, gx: f64, gy: f64, gz: f64) -> StreamableData {
        StreamableData::Imu(ImuData {
            sender_id: sender.into(),
            timestamp: SystemTime::now(),
            gyroscope: Vec3d::new(gx, gy, gz),
            accelerometer: Vec3d::new(0.0, 0.0, 9.81),
            ..Default::default()
        })
    }

    fn collect_output(filter: &ScriptFilter) -> Arc<Mutex<Vec<StreamableData>>> {
        let collected = Arc::new(Mutex::new(Vec::new()));
        let c = collected.clone();
        filter.set_on_output(Box::new(move |data| {
            c.lock().unwrap().push(data);
        }));
        collected
    }

    #[test]
    fn passthrough_script() {
        let mut f = ScriptFilter::new("test", json!({}));
        let out = collect_output(&f);
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        let results = out.lock().unwrap();
        assert_eq!(results.len(), 1);
        if let StreamableData::Imu(ref d) = results[0] {
            assert_eq!(d.sender_id, "imu0");
            assert!((d.gyroscope.x - 1.0).abs() < 1e-10);
            assert!((d.gyroscope.y - 2.0).abs() < 1e-10);
            assert!((d.gyroscope.z - 3.0).abs() < 1e-10);
        } else {
            panic!("Expected Imu output");
        }
    }

    #[test]
    fn modify_gyroscope() {
        let script = r#"
            fn process(data) {
                data.gyroscope = vec3_scale(data.gyroscope, 2.0);
                data
            }
        "#;
        let mut f = ScriptFilter::new("test", json!({"script": script}));
        let out = collect_output(&f);
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        let results = out.lock().unwrap();
        assert_eq!(results.len(), 1);
        if let StreamableData::Imu(ref d) = results[0] {
            assert!((d.gyroscope.x - 2.0).abs() < 1e-10);
            assert!((d.gyroscope.y - 4.0).abs() < 1e-10);
            assert!((d.gyroscope.z - 6.0).abs() < 1e-10);
        } else {
            panic!("Expected Imu output");
        }
    }

    #[test]
    fn drop_message_returns_unit() {
        let script = r#"
            fn process(data) {
                // Return nothing to drop
            }
        "#;
        let mut f = ScriptFilter::new("test", json!({"script": script}));
        let out = collect_output(&f);
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        assert_eq!(out.lock().unwrap().len(), 0);
    }

    #[test]
    fn compile_error_keeps_old_script() {
        let mut f = ScriptFilter::new("test", json!({}));
        assert!(f.m_ast.is_some());

        f.update_script("fn process(data) { THIS IS INVALID +++");
        // Old AST should be preserved
        assert!(f.m_ast.is_some());
        assert!(f.m_last_error.is_some());

        // Old script still works
        let out = collect_output(&f);
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        assert_eq!(out.lock().unwrap().len(), 1);
    }

    #[test]
    fn runtime_error_logs_but_continues() {
        let script = r#"
            fn process(data) {
                let x = 1 / 0;
                data
            }
        "#;
        let mut f = ScriptFilter::new("test", json!({"script": script}));
        let out = collect_output(&f);

        // Should not panic
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        assert!(f.m_last_error.is_some());
        // No output emitted on error
        assert_eq!(out.lock().unwrap().len(), 0);

        // Filter should still be alive for next call
        assert_eq!(f.m_frame_count, 1);
    }

    #[test]
    fn type_conversion() {
        let script = r#"
            fn process(data) {
                let out = #{};
                out._type = "FusedPose";
                out.sender_id = data.sender_id;
                out.timestamp = data.timestamp;
                out.transmission_time = data.timestamp;
                out.last_data_time = data.timestamp;
                out.position = vec3(0.0, 0.0, 0.0);
                out.orientation = quat(1.0, 0.0, 0.0, 0.0);
                out.angular_velocity = data.gyroscope;
                out.velocity = vec3(0.0, 0.0, 0.0);
                out.acceleration = data.accelerometer;
                out.frame_number = 0;
                out
            }
        "#;
        let mut f = ScriptFilter::new("test", json!({"script": script}));
        let out = collect_output(&f);
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        let results = out.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], StreamableData::FusedPose(_)));
        if let StreamableData::FusedPose(ref d) = results[0] {
            assert_eq!(d.sender_id, "imu0");
            assert!((d.angular_velocity.x - 1.0).abs() < 1e-10);
        }
    }

    #[test]
    fn status_reports() {
        let mut f = ScriptFilter::new("test", json!({}));
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        let s = f.status();
        assert_eq!(s["frameCount"], 2);
        assert_eq!(s["scriptCompiled"], true);
        assert!(s["lastError"].is_null());
    }

    #[test]
    fn stateful_script() {
        let script = r#"
            fn process(data) {
                if state.keys().len() == 0 {
                    state.count = 0;
                }
                state.count += 1;
                data.gyroscope.x = state.count.to_float();
                data
            }
        "#;
        let mut f = ScriptFilter::new("test", json!({"script": script}));
        let out = collect_output(&f);
        f.receive_data(make_imu("imu0", 0.0, 0.0, 0.0));
        f.receive_data(make_imu("imu0", 0.0, 0.0, 0.0));
        f.receive_data(make_imu("imu0", 0.0, 0.0, 0.0));

        let results = out.lock().unwrap();
        assert_eq!(results.len(), 3);
        if let StreamableData::Imu(ref d) = results[2] {
            assert!((d.gyroscope.x - 3.0).abs() < 1e-10);
        } else {
            panic!("Expected Imu");
        }
    }

    #[test]
    fn live_script_update() {
        let mut f = ScriptFilter::new("test", json!({}));
        let out = collect_output(&f);

        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        assert_eq!(out.lock().unwrap().len(), 1);

        // Update to a script that drops all messages
        let cmd = ApiRequest::new(
            "setConfigJsonPath",
            "/sinks/test",
            json!({"script": "fn process(data) { }"}),
            "",
        );
        f.process_command(&cmd);

        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        // Still only 1 from before the update
        assert_eq!(out.lock().unwrap().len(), 1);
    }

    #[test]
    fn disabled_blocks_data() {
        let mut f = ScriptFilter::new("test", json!({}));
        let out = collect_output(&f);
        f.set_enabled(false);
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        assert_eq!(out.lock().unwrap().len(), 0);
        assert_eq!(f.m_frame_count, 0);
    }

    #[test]
    fn max_operations_prevents_infinite_loop() {
        let script = r#"
            fn process(data) {
                loop { }
                data
            }
        "#;
        let mut f = ScriptFilter::new("test", json!({"script": script}));
        let out = collect_output(&f);
        f.receive_data(make_imu("imu0", 1.0, 2.0, 3.0));
        assert!(f.m_last_error.is_some());
        assert_eq!(out.lock().unwrap().len(), 0);
    }

    #[test]
    fn unmapped_data_type_passes_through() {
        let mut f = ScriptFilter::new("test", json!({}));
        let out = collect_output(&f);
        f.receive_data(StreamableData::Reset);
        // Unmapped types pass through unchanged
        assert_eq!(out.lock().unwrap().len(), 1);
        assert!(matches!(out.lock().unwrap()[0], StreamableData::Reset));
        assert_eq!(f.m_frame_count, 0);
    }

    #[test]
    fn vehicle_speed_roundtrip() {
        let script = r#"
            fn process(data) {
                data.linear = data.linear * 3.6;
                data
            }
        "#;
        let mut f = ScriptFilter::new("test", json!({"script": script}));
        let out = collect_output(&f);
        f.receive_data(StreamableData::VehicleSpeed(VehicleSpeed {
            sender_id: "speed0".into(),
            linear: 10.0,
            angular: 0.5,
            valid_angular: true,
            ..Default::default()
        }));
        let results = out.lock().unwrap();
        assert_eq!(results.len(), 1);
        if let StreamableData::VehicleSpeed(ref d) = results[0] {
            assert!((d.linear - 36.0).abs() < 1e-10);
            assert!((d.angular - 0.5).abs() < 1e-10);
            assert!(d.valid_angular);
        } else {
            panic!("Expected VehicleSpeed");
        }
    }
}
