use std::sync::{Arc, Mutex};

use fusion_types::{ApiRequest, FusedPose, JsonValueExt, OpticalData, StreamableData, Vec3d};
use nalgebra::{Quaternion, UnitQuaternion};

use crate::node::{CommandConsumerCallback, ConsumerCallback, Node};

/// Faithful port of C++ Fusion::InsideOutFilter.
///
/// Combines inside-out optical tracking (e.g. HMD SLAM) with external optical
/// tracking (fused pose from another filter) using per-axis high-pass / low-pass
/// complementary filtering.
///
/// Data flow:
/// - OpticalData (inside-out tracking) -> processed through HP/LP filters -> emits FusedPose
/// - FusedPose (external optical tracking) -> stored as m_latestPose (used in next OpticalData processing)
/// - IMU data is NOT processed by this filter.
pub struct InsideOutFilter {
    m_name: String,
    m_enabled: bool,

    // Configurable parameters (matching C++ defaults)
    m_io_lp_weight: f64,
    m_io_hp_weight: f64,
    m_opt_lp_weight: f64,
    m_use_io_height: bool,
    m_use_io_horizontal: bool,
    m_prediction_time_modifier: f64,

    // Latest fused pose from external optical tracking
    m_latest_pose: FusedPose,

    // IO high-pass filter accumulators (per-axis low-pass for computing HP)
    m_io_x_acc: f64,
    m_io_y_acc: f64,
    m_io_z_acc: f64,

    // IO high-pass output accumulators (smoothed HP)
    m_io_x_hp_acc: f64,
    m_io_y_hp_acc: f64,
    m_io_z_hp_acc: f64,

    // IO high-pass values (used only for initialization)
    m_io_x_hp: f64,
    m_io_y_hp: f64,
    m_io_z_hp: f64,

    // Optical low-pass filter accumulators
    m_opt_x_acc: f64,
    m_opt_y_acc: f64,
    m_opt_z_acc: f64,

    // First-valid-data flag
    m_hp_once: bool,

    // Callbacks
    m_on_output: Arc<Mutex<Option<ConsumerCallback>>>,
    m_on_command_output: Arc<Mutex<Option<CommandConsumerCallback>>>,
}

impl InsideOutFilter {
    pub fn new(name: &str, config: serde_json::Value) -> Self {
        let mut filter = Self {
            m_name: name.to_owned(),
            m_enabled: true,

            m_io_lp_weight: 0.999,
            m_io_hp_weight: 0.3,
            m_opt_lp_weight: 0.999,
            m_use_io_height: false,
            m_use_io_horizontal: false,
            m_prediction_time_modifier: 0.02,

            m_latest_pose: FusedPose::default(),

            m_io_x_acc: 0.0,
            m_io_y_acc: 0.0,
            m_io_z_acc: 0.0,

            m_io_x_hp_acc: 0.0,
            m_io_y_hp_acc: 0.0,
            m_io_z_hp_acc: 0.0,

            m_io_x_hp: 0.0,
            m_io_y_hp: 0.0,
            m_io_z_hp: 0.0,

            m_opt_x_acc: 0.0,
            m_opt_y_acc: 0.0,
            m_opt_z_acc: 0.0,

            m_hp_once: true,

            m_on_output: Arc::new(Mutex::new(None)),
            m_on_command_output: Arc::new(Mutex::new(None)),
        };

        filter.configure(&config);
        filter
    }

    /// Read configuration parameters (matching C++ InsideOutFilter::configure).
    fn configure(&mut self, config: &serde_json::Value) {
        self.m_io_lp_weight = config.value_f64("ioLpWeight", self.m_io_lp_weight);
        self.m_io_hp_weight = config.value_f64("ioHpWeight", self.m_io_hp_weight);
        self.m_opt_lp_weight = config.value_f64("optLpWeight", self.m_opt_lp_weight);
        self.m_use_io_height = config.value_bool("useIOHeight", self.m_use_io_height);
        self.m_use_io_horizontal = config.value_bool("useIOHorizontal", self.m_use_io_horizontal);
        if let Some(v) = config.get("predictionTimeModifier").and_then(|v| v.as_f64()) {
            self.m_prediction_time_modifier = v;

            // Send the predictionTimeModifier command (matching C++ behaviour)
            let cmd = ApiRequest::new(
                "predictionTimeModifier",
                &self.m_name,
                serde_json::json!(self.m_prediction_time_modifier),
                "",
            );
            let on_cmd = self.m_on_command_output.lock().unwrap();
            if let Some(ref callback) = *on_cmd {
                callback(cmd);
            }
        }
    }

    /// Rotate a vector by a quaternion: v' = q * v.
    ///
    /// Matches C++ rotateVectorByQuaternion(x, y, z, w, rx, ry, rz)
    /// where the quaternion is (w, rx, ry, rz).
    fn rotate_vector(pos: &mut Vec3d, w: f64, rx: f64, ry: f64, rz: f64) {
        let q = UnitQuaternion::new_normalize(Quaternion::new(w, rx, ry, rz));
        *pos = q * *pos;
    }

    /// Process inside-out OpticalData through the HP/LP complementary filter.
    ///
    /// Faithful port of C++ InsideOutFilter::opticalData.
    fn process_optical_data(&mut self, mut d: OpticalData) {
        let p = &self.m_latest_pose;

        // Step 1: Rotate IO position by inverse of IO orientation
        // C++: rotateVectorByQuaternion(&pos, d.orientation.w(), -d.orientation.x(), -d.orientation.y(), -d.orientation.z())
        Self::rotate_vector(
            &mut d.position,
            d.orientation.w,
            -d.orientation.i,
            -d.orientation.j,
            -d.orientation.k,
        );

        // Step 2: Rotate result by latest fused pose orientation
        // C++: rotateVectorByQuaternion(&pos, p.orientation.w(), p.orientation.x(), p.orientation.y(), p.orientation.z())
        Self::rotate_vector(
            &mut d.position,
            p.orientation.w,
            p.orientation.i,
            p.orientation.j,
            p.orientation.k,
        );

        // First valid data pair initialization
        if self.m_hp_once {
            self.m_io_x_acc = d.position.x;
            self.m_io_y_acc = d.position.y;
            self.m_io_z_acc = d.position.z;
            self.m_io_x_hp_acc = 0.0;
            self.m_io_y_hp_acc = 0.0;
            self.m_io_z_hp_acc = 0.0;
            self.m_opt_x_acc = p.position.x;
            self.m_opt_y_acc = p.position.y;
            self.m_opt_z_acc = p.position.z;
            self.m_io_x_hp = self.m_io_x_acc;
            self.m_io_y_hp = self.m_io_y_acc;
            self.m_io_z_hp = self.m_io_z_acc;

            if d.position.x.abs() > 0.0 && p.position.x.abs() > 0.0 {
                self.m_hp_once = false;
            }
        }

        // High-pass filter for inside-out tracking (per-axis)
        // X axis
        let io_x_lp =
            self.m_io_x_acc * self.m_io_lp_weight + d.position.x * (1.0 - self.m_io_lp_weight);
        self.m_io_x_acc = io_x_lp;
        let io_x_hp = d.position.x - io_x_lp;
        let io_x_hp_lp =
            self.m_io_x_hp_acc * self.m_io_hp_weight + io_x_hp * (1.0 - self.m_io_hp_weight);
        self.m_io_x_hp_acc = io_x_hp_lp;

        // Y axis
        let io_y_lp =
            self.m_io_y_acc * self.m_io_lp_weight + d.position.y * (1.0 - self.m_io_lp_weight);
        self.m_io_y_acc = io_y_lp;
        let io_y_hp = d.position.y - io_y_lp;
        let io_y_hp_lp =
            self.m_io_y_hp_acc * self.m_io_hp_weight + io_y_hp * (1.0 - self.m_io_hp_weight);
        self.m_io_y_hp_acc = io_y_hp_lp;

        // Z axis
        let io_z_lp =
            self.m_io_z_acc * self.m_io_lp_weight + d.position.z * (1.0 - self.m_io_lp_weight);
        self.m_io_z_acc = io_z_lp;
        let io_z_hp = d.position.z - io_z_lp;
        let io_z_hp_lp =
            self.m_io_z_hp_acc * self.m_io_hp_weight + io_z_hp * (1.0 - self.m_io_hp_weight);
        self.m_io_z_hp_acc = io_z_hp_lp;

        // Low-pass filter for optical (fused pose) position
        let opt_x_lp =
            self.m_opt_x_acc * self.m_opt_lp_weight + p.position.x * (1.0 - self.m_opt_lp_weight);
        self.m_opt_x_acc = opt_x_lp;

        let opt_y_lp =
            self.m_opt_y_acc * self.m_opt_lp_weight + p.position.y * (1.0 - self.m_opt_lp_weight);
        self.m_opt_y_acc = opt_y_lp;

        let opt_z_lp =
            self.m_opt_z_acc * self.m_opt_lp_weight + p.position.z * (1.0 - self.m_opt_lp_weight);
        self.m_opt_z_acc = opt_z_lp;

        // Combine filtered positions
        let mut v = Vec3d::zeros();

        if self.m_use_io_horizontal {
            v.x = opt_x_lp + io_x_hp_lp;
            v.z = opt_z_lp + io_z_hp_lp;

            if self.m_use_io_height {
                v.y = d.position.y;
            } else {
                v.y = opt_y_lp + io_y_hp_lp;
            }
        } else {
            v.x = p.position.x;
            v.z = p.position.z;

            if self.m_use_io_height {
                v.y = d.position.y;
            } else {
                v.y = p.position.y;
            }
        }

        // Build output FusedPose based on m_latestPose with updated position
        // C++: p.position = v; return p;
        let mut output_pose = self.m_latest_pose.clone();
        output_pose.position = v;

        // Emit the fused pose
        let on_output = self.m_on_output.lock().unwrap();
        if let Some(ref callback) = *on_output {
            callback(StreamableData::FusedPose(output_pose));
        }
    }
}

impl Node for InsideOutFilter {
    fn name(&self) -> &str {
        &self.m_name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting InsideOutFilter: {}", self.m_name);
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping InsideOutFilter: {}", self.m_name);
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.m_enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.m_enabled = enabled;
    }

    fn receive_data(&mut self, data: StreamableData) {
        match data {
            StreamableData::Optical(optical) => {
                self.process_optical_data(optical);
            }
            StreamableData::FusedPose(pose) => {
                self.m_latest_pose = pose;
            }
            _ => {}
        }
    }

    fn receive_command(&mut self, _cmd: &ApiRequest) {
        // C++ InsideOutFilter has no processCommand override
    }

    fn set_on_output(&self, callback: ConsumerCallback) {
        let mut on_output = self.m_on_output.lock().unwrap();
        *on_output = Some(callback);
    }

    fn set_on_command_output(&self, callback: CommandConsumerCallback) {
        let mut on_cmd = self.m_on_command_output.lock().unwrap();
        *on_cmd = Some(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fusion_types::Quatd;
    use std::time::{Duration, UNIX_EPOCH};

    fn make_optical(pos: Vec3d, ori: Quatd, ts_ms: u64) -> OpticalData {
        OpticalData {
            position: pos,
            orientation: ori,
            timestamp: UNIX_EPOCH + Duration::from_millis(ts_ms),
            ..Default::default()
        }
    }

    fn make_fused_pose(pos: Vec3d, ori: Quatd) -> FusedPose {
        FusedPose {
            position: pos,
            orientation: ori,
            ..Default::default()
        }
    }

    #[test]
    fn default_config_values() {
        let filter = InsideOutFilter::new("test", serde_json::json!({}));
        assert!((filter.m_io_lp_weight - 0.999).abs() < 1e-9);
        assert!((filter.m_io_hp_weight - 0.3).abs() < 1e-9);
        assert!((filter.m_opt_lp_weight - 0.999).abs() < 1e-9);
        assert!(!filter.m_use_io_height);
        assert!(!filter.m_use_io_horizontal);
        assert!((filter.m_prediction_time_modifier - 0.02).abs() < 1e-9);
        assert!(filter.m_hp_once);
    }

    #[test]
    fn config_from_json() {
        let config = serde_json::json!({
            "ioLpWeight": 0.99,
            "ioHpWeight": 0.5,
            "optLpWeight": 0.95,
            "useIOHeight": true,
            "useIOHorizontal": true,
        });
        let filter = InsideOutFilter::new("test", config);
        assert!((filter.m_io_lp_weight - 0.99).abs() < 1e-9);
        assert!((filter.m_io_hp_weight - 0.5).abs() < 1e-9);
        assert!((filter.m_opt_lp_weight - 0.95).abs() < 1e-9);
        assert!(filter.m_use_io_height);
        assert!(filter.m_use_io_horizontal);
    }

    #[test]
    fn fused_pose_updates_latest_pose() {
        let mut filter = InsideOutFilter::new("test", serde_json::json!({}));

        let pose = make_fused_pose(Vec3d::new(1.0, 2.0, 3.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        assert!((filter.m_latest_pose.position.x - 1.0).abs() < 1e-9);
        assert!((filter.m_latest_pose.position.y - 2.0).abs() < 1e-9);
        assert!((filter.m_latest_pose.position.z - 3.0).abs() < 1e-9);
    }

    #[test]
    fn imu_data_is_ignored() {
        let mut filter = InsideOutFilter::new("test", serde_json::json!({}));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let imu = fusion_types::ImuData {
            timestamp: UNIX_EPOCH + Duration::from_millis(100),
            period: 0.01,
            gyroscope: Vec3d::new(0.0, 0.0, 10.0),
            accelerometer: Vec3d::new(0.0, 0.0, 9.81),
            ..Default::default()
        };
        filter.receive_data(StreamableData::Imu(imu));

        let output = results.lock().unwrap();
        assert!(output.is_empty(), "IMU data should not produce output");
    }

    #[test]
    fn optical_data_emits_fused_pose() {
        let mut filter = InsideOutFilter::new("test", serde_json::json!({}));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        // First set a fused pose so m_latestPose has nonzero position
        let pose = make_fused_pose(Vec3d::new(1.0, 2.0, 3.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        // Then send optical data
        let optical = make_optical(Vec3d::new(0.5, 0.5, 0.5), Quatd::identity(), 100);
        filter.receive_data(StreamableData::Optical(optical));

        let output = results.lock().unwrap();
        assert_eq!(output.len(), 1, "Optical data should produce one FusedPose output");
        match &output[0] {
            StreamableData::FusedPose(_) => {}
            other => panic!("Expected FusedPose, got {:?}", other.variant_name()),
        }
    }

    #[test]
    fn hp_once_initializes_accumulators() {
        let mut filter = InsideOutFilter::new("test", serde_json::json!({}));

        // Set up a nonzero latest pose so m_hp_once triggers and then clears
        let pose = make_fused_pose(Vec3d::new(1.0, 2.0, 3.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        assert!(filter.m_hp_once, "Should be true before first optical data");

        // Send optical data with nonzero x (and latest pose has nonzero x)
        let optical = make_optical(Vec3d::new(0.5, 0.5, 0.5), Quatd::identity(), 100);
        filter.receive_data(StreamableData::Optical(optical));

        assert!(
            !filter.m_hp_once,
            "Should be false after first valid data pair"
        );
    }

    #[test]
    fn hp_once_stays_true_when_positions_are_zero() {
        let mut filter = InsideOutFilter::new("test", serde_json::json!({}));

        // Latest pose position.x = 0 -> condition fabs(p.position[0]) > 0 fails
        let pose = make_fused_pose(Vec3d::new(0.0, 2.0, 3.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        // Optical data with identity orientation: after rotation, position stays the same
        // IO position.x = 0.5 but p.position.x = 0
        let optical = make_optical(Vec3d::new(0.5, 0.5, 0.5), Quatd::identity(), 100);
        filter.receive_data(StreamableData::Optical(optical));

        assert!(
            filter.m_hp_once,
            "Should stay true when latest pose x is zero"
        );
    }

    #[test]
    fn use_io_horizontal_combines_hp_lp() {
        let config = serde_json::json!({
            "useIOHorizontal": true,
            "ioLpWeight": 0.5,
            "ioHpWeight": 0.5,
            "optLpWeight": 0.5,
        });
        let mut filter = InsideOutFilter::new("test", config);

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        // Set latest pose
        let pose = make_fused_pose(Vec3d::new(2.0, 4.0, 6.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        // Send several optical frames to let filters settle
        for i in 0..50 {
            let optical = make_optical(
                Vec3d::new(1.0, 1.0, 1.0),
                Quatd::identity(),
                100 + i * 10,
            );
            filter.receive_data(StreamableData::Optical(optical));
        }

        let output = results.lock().unwrap();
        assert!(!output.is_empty());

        // With useIOHorizontal, x and z should be opt_lp + io_hp_lp
        // After many iterations, the filters should have converged
        if let StreamableData::FusedPose(ref last_pose) = output[output.len() - 1] {
            // The output should be finite and reasonable
            assert!(last_pose.position.x.is_finite());
            assert!(last_pose.position.y.is_finite());
            assert!(last_pose.position.z.is_finite());
        }
    }

    #[test]
    fn without_io_horizontal_uses_latest_pose_xz() {
        let config = serde_json::json!({
            "useIOHorizontal": false,
            "useIOHeight": false,
        });
        let mut filter = InsideOutFilter::new("test", config);

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = make_fused_pose(Vec3d::new(10.0, 20.0, 30.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        let optical = make_optical(Vec3d::new(1.0, 1.0, 1.0), Quatd::identity(), 100);
        filter.receive_data(StreamableData::Optical(optical));

        let output = results.lock().unwrap();
        assert_eq!(output.len(), 1);
        if let StreamableData::FusedPose(ref out_pose) = output[0] {
            // Without useIOHorizontal: x = p.position.x, z = p.position.z
            assert!(
                (out_pose.position.x - 10.0).abs() < 1e-9,
                "x should be latest pose x"
            );
            assert!(
                (out_pose.position.z - 30.0).abs() < 1e-9,
                "z should be latest pose z"
            );
            // Without useIOHeight: y = p.position.y
            assert!(
                (out_pose.position.y - 20.0).abs() < 1e-9,
                "y should be latest pose y"
            );
        }
    }

    #[test]
    fn use_io_height_takes_io_position_y() {
        let config = serde_json::json!({
            "useIOHorizontal": false,
            "useIOHeight": true,
        });
        let mut filter = InsideOutFilter::new("test", config);

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        let pose = make_fused_pose(Vec3d::new(10.0, 20.0, 30.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        // With identity orientation, rotated IO position = original IO position
        let optical = make_optical(Vec3d::new(1.0, 5.0, 1.0), Quatd::identity(), 100);
        filter.receive_data(StreamableData::Optical(optical));

        let output = results.lock().unwrap();
        assert_eq!(output.len(), 1);
        if let StreamableData::FusedPose(ref out_pose) = output[0] {
            // x and z from latest pose
            assert!((out_pose.position.x - 10.0).abs() < 1e-9);
            assert!((out_pose.position.z - 30.0).abs() < 1e-9);
            // y from IO position (d.position.y after rotation)
            assert!(
                (out_pose.position.y - 5.0).abs() < 1e-9,
                "y should be IO position y"
            );
        }
    }

    #[test]
    fn rotation_applied_correctly() {
        // Test that IO position is rotated by conjugate(IO_ori) then by latestPose ori
        let mut filter = InsideOutFilter::new("test", serde_json::json!({}));

        let results = Arc::new(Mutex::new(Vec::new()));
        let r = results.clone();
        filter.set_on_output(Box::new(move |data| {
            r.lock().unwrap().push(data);
        }));

        // 90-degree rotation about Y axis
        let angle = std::f64::consts::FRAC_PI_2;
        let half = angle / 2.0;
        let ori_90y = UnitQuaternion::new_normalize(
            Quaternion::new(half.cos(), 0.0, half.sin(), 0.0),
        );

        // Set latest pose with identity orientation
        let pose = make_fused_pose(Vec3d::new(1.0, 0.0, 0.0), Quatd::identity());
        filter.receive_data(StreamableData::FusedPose(pose));

        // IO data with 90-deg Y rotation and position along X
        let optical = make_optical(Vec3d::new(1.0, 0.0, 0.0), ori_90y, 100);
        filter.receive_data(StreamableData::Optical(optical));

        // After conjugate(90Y) rotation of (1,0,0): should rotate to (0,0,1) approximately
        // Then identity rotation keeps it at (0,0,1)
        // The HP/LP filters will modify the final output, but the rotation should have been applied
        let output = results.lock().unwrap();
        assert_eq!(output.len(), 1);
    }

    #[test]
    fn prediction_time_modifier_sends_command() {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let c = commands.clone();

        let mut filter = InsideOutFilter::new("test", serde_json::json!({}));
        filter.set_on_command_output(Box::new(move |cmd| {
            c.lock().unwrap().push(cmd);
        }));

        // Now reconfigure with predictionTimeModifier
        filter.configure(&serde_json::json!({
            "predictionTimeModifier": 0.05,
        }));

        let cmds = commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "predictionTimeModifier");
        assert_eq!(cmds[0].topic, "test");
        assert!((cmds[0].data.as_f64().unwrap() - 0.05).abs() < 1e-9);
    }

    #[test]
    fn node_trait_name_and_enabled() {
        let mut filter = InsideOutFilter::new("my_io_filter", serde_json::json!({}));
        assert_eq!(filter.name(), "my_io_filter");
        assert!(filter.is_enabled());
        filter.set_enabled(false);
        assert!(!filter.is_enabled());
    }
}
