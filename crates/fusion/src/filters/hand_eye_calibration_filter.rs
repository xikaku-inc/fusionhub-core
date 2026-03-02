use std::sync::{Arc, Mutex};

use fusion_types::{ApiRequest, OpticalData, Quatd, StreamableData, Vec3d};

use crate::node::{ConsumerCallback, Node};

/// Calibration state for hand-eye calibration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandEyeCalibrationState {
    Idle,
    Collecting,
    Solving,
    Finished,
    Failed,
}

/// Collected motion pair for hand-eye calibration (AX = XB problem).
#[derive(Clone, Debug)]
struct MotionPair {
    rotation_a: Quatd,
    translation_a: Vec3d,
    rotation_b: Quatd,
    translation_b: Vec3d,
}

/// Hand-eye calibration result.
#[derive(Clone, Debug)]
pub struct HandEyeCalibrationResult {
    pub rotation: Quatd,
    pub translation: Vec3d,
    pub n_pairs: usize,
    pub finished: bool,
    pub error: f64,
}

impl Default for HandEyeCalibrationResult {
    fn default() -> Self {
        Self {
            rotation: Quatd::identity(),
            translation: Vec3d::zeros(),
            n_pairs: 0,
            finished: false,
            error: f64::MAX,
        }
    }
}

/// Hand-eye calibration filter between two optical tracking sources.
///
/// Collects paired motion samples from two optical tracking systems and
/// solves the AX = XB hand-eye calibration problem to find the rigid
/// transformation between them.
///
/// NOTE: The solver is a stub. A full implementation would require an
/// SVD-based solution (e.g., Tsai-Lenz or Park-Martin method), which
/// depends on an equivalent of OpenCV's calibrateHandEye.
pub struct HandEyeCalibrationFilter {
    m_name: String,
    m_enabled: bool,
    m_source_a_id: String,
    m_source_b_id: String,
    m_state: HandEyeCalibrationState,
    m_result: HandEyeCalibrationResult,
    m_motion_pairs: Vec<MotionPair>,
    m_last_optical_a: Option<OpticalData>,
    m_last_optical_b: Option<OpticalData>,
    m_prev_optical_a: Option<OpticalData>,
    m_prev_optical_b: Option<OpticalData>,
    m_min_pairs: usize,
    m_max_pairs: usize,
    m_min_rotation_deg: f64,
    m_on_output: Arc<Mutex<Option<ConsumerCallback>>>,
}

impl HandEyeCalibrationFilter {
    pub fn new(config: serde_json::Value) -> Self {
        Self {
            m_name: config
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("HandEyeCalibrationFilter")
                .to_owned(),
            m_enabled: true,
            m_source_a_id: config
                .get("sourceAId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
            m_source_b_id: config
                .get("sourceBId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
            m_state: HandEyeCalibrationState::Idle,
            m_result: HandEyeCalibrationResult::default(),
            m_motion_pairs: Vec::new(),
            m_last_optical_a: None,
            m_last_optical_b: None,
            m_prev_optical_a: None,
            m_prev_optical_b: None,
            m_min_pairs: config
                .get("minPairs")
                .and_then(|v| v.as_u64())
                .unwrap_or(20) as usize,
            m_max_pairs: config
                .get("maxPairs")
                .and_then(|v| v.as_u64())
                .unwrap_or(200) as usize,
            m_min_rotation_deg: config
                .get("minRotationDeg")
                .and_then(|v| v.as_f64())
                .unwrap_or(5.0),
            m_on_output: Arc::new(Mutex::new(None)),
        }
    }

    pub fn name(&self) -> &str {
        &self.m_name
    }

    pub fn state(&self) -> &HandEyeCalibrationState {
        &self.m_state
    }

    pub fn result(&self) -> &HandEyeCalibrationResult {
        &self.m_result
    }

    pub fn set_on_output(&self, callback: ConsumerCallback) {
        let mut on_output = self.m_on_output.lock().unwrap();
        *on_output = Some(callback);
    }

    pub fn process_data(&mut self, data: StreamableData) {
        if self.m_state != HandEyeCalibrationState::Collecting {
            return;
        }

        if let StreamableData::Optical(optical) = data {
            if optical.sender_id == self.m_source_a_id {
                self.m_prev_optical_a = self.m_last_optical_a.take();
                self.m_last_optical_a = Some(optical);
            } else if optical.sender_id == self.m_source_b_id {
                self.m_prev_optical_b = self.m_last_optical_b.take();
                self.m_last_optical_b = Some(optical);
            }

            self.try_collect_pair();
        }
    }

    pub fn process_command(&mut self, cmd: &ApiRequest) {
        match cmd.command.as_str() {
            "start" | "startCalibration" => {
                self.m_motion_pairs.clear();
                self.m_state = HandEyeCalibrationState::Collecting;
                self.m_result = HandEyeCalibrationResult::default();
                self.m_last_optical_a = None;
                self.m_last_optical_b = None;
                self.m_prev_optical_a = None;
                self.m_prev_optical_b = None;
                log::info!("{}: calibration started, collecting motion pairs", self.m_name);
            }
            "stop" | "stopCalibration" => {
                if self.m_state == HandEyeCalibrationState::Collecting {
                    self.m_state = HandEyeCalibrationState::Idle;
                    log::info!("{}: calibration stopped", self.m_name);
                }
            }
            "solve" => {
                self.solve();
            }
            "getResult" => {
                self.emit_result();
            }
            "reset" => {
                self.m_motion_pairs.clear();
                self.m_state = HandEyeCalibrationState::Idle;
                self.m_result = HandEyeCalibrationResult::default();
                log::info!("{}: calibration reset", self.m_name);
            }
            _ => {
                log::trace!("{}: unhandled command '{}'", self.m_name, cmd.command);
            }
        }
    }

    fn try_collect_pair(&mut self) {
        let (prev_a, curr_a, prev_b, curr_b) = match (
            &self.m_prev_optical_a,
            &self.m_last_optical_a,
            &self.m_prev_optical_b,
            &self.m_last_optical_b,
        ) {
            (Some(pa), Some(ca), Some(pb), Some(cb)) => (pa, ca, pb, cb),
            _ => return,
        };

        // Compute relative motions
        let rot_a = curr_a.orientation * prev_a.orientation.inverse();
        let trans_a = curr_a.position - prev_a.position;
        let rot_b = curr_b.orientation * prev_b.orientation.inverse();
        let trans_b = curr_b.position - prev_b.position;

        // Only collect if there was sufficient rotation
        let angle_a_deg = rot_a.angle().to_degrees();
        if angle_a_deg < self.m_min_rotation_deg {
            return;
        }

        self.m_motion_pairs.push(MotionPair {
            rotation_a: rot_a,
            translation_a: trans_a,
            rotation_b: rot_b,
            translation_b: trans_b,
        });

        log::debug!(
            "{}: collected pair {} (rotation={:.1} deg)",
            self.m_name,
            self.m_motion_pairs.len(),
            angle_a_deg,
        );

        if self.m_motion_pairs.len() >= self.m_max_pairs {
            self.solve();
        } else if self.m_motion_pairs.len() >= self.m_min_pairs {
            // Attempt an intermediate solve
            self.solve();
        }
    }

    fn solve(&mut self) {
        if self.m_motion_pairs.len() < 3 {
            log::warn!(
                "{}: not enough motion pairs for calibration ({})",
                self.m_name,
                self.m_motion_pairs.len()
            );
            self.m_state = HandEyeCalibrationState::Failed;
            return;
        }

        self.m_state = HandEyeCalibrationState::Solving;
        log::info!(
            "{}: solving hand-eye calibration with {} pairs",
            self.m_name,
            self.m_motion_pairs.len()
        );

        // Stub solver: average the rotation offsets as an approximation.
        // A proper implementation would use Tsai-Lenz, Park-Martin, or similar.
        let n = self.m_motion_pairs.len() as f64;
        let mut sum_w = 0.0;
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_z = 0.0;
        let mut avg_translation = Vec3d::zeros();

        for pair in &self.m_motion_pairs {
            // X ~= B * A^-1 (very rough approximation)
            let x_approx = pair.rotation_b * pair.rotation_a.inverse();
            let q = x_approx.into_inner();

            // Ensure consistent hemisphere for averaging
            let sign = if sum_w * q.w + sum_x * q.i + sum_y * q.j + sum_z * q.k < 0.0 {
                -1.0
            } else {
                1.0
            };

            sum_w += sign * q.w;
            sum_x += sign * q.i;
            sum_y += sign * q.j;
            sum_z += sign * q.k;

            avg_translation += pair.translation_b - pair.rotation_b * pair.translation_a;
        }

        let norm = (sum_w * sum_w + sum_x * sum_x + sum_y * sum_y + sum_z * sum_z).sqrt();
        if norm < 1e-12 {
            self.m_state = HandEyeCalibrationState::Failed;
            log::warn!("{}: degenerate solution", self.m_name);
            return;
        }

        let result_quat = nalgebra::UnitQuaternion::from_quaternion(
            nalgebra::Quaternion::new(sum_w / norm, sum_x / norm, sum_y / norm, sum_z / norm),
        );

        avg_translation /= n;

        self.m_result = HandEyeCalibrationResult {
            rotation: result_quat,
            translation: avg_translation,
            n_pairs: self.m_motion_pairs.len(),
            finished: true,
            error: 0.0, // Stub: no error metric computed
        };

        self.m_state = HandEyeCalibrationState::Finished;
        log::info!(
            "{}: calibration finished, rotation: {:?}, translation: {:?}",
            self.m_name,
            self.m_result.rotation,
            self.m_result.translation
        );

        self.emit_result();
    }

    fn emit_result(&self) {
        let q = &self.m_result.rotation;
        let t = &self.m_result.translation;

        log::info!(
            "{}: result - rotation: (w={:.4}, x={:.4}, y={:.4}, z={:.4}), translation: ({:.4}, {:.4}, {:.4}), pairs: {}",
            self.m_name,
            q.w, q.i, q.j, q.k,
            t.x, t.y, t.z,
            self.m_result.n_pairs,
        );

        let on_output = self.m_on_output.lock().unwrap();
        if let Some(ref callback) = *on_output {
            let ts = fusion_types::Timestamp::current();
            callback(StreamableData::Timestamp(ts));
        }
    }
}

impl Node for HandEyeCalibrationFilter {
    fn name(&self) -> &str {
        &self.m_name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        log::info!("Starting HandEyeCalibrationFilter: {}", self.m_name);
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        log::info!("Stopping HandEyeCalibrationFilter: {}", self.m_name);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_idle() {
        let filter = HandEyeCalibrationFilter::new(serde_json::json!({}));
        assert_eq!(*filter.state(), HandEyeCalibrationState::Idle);
    }

    #[test]
    fn start_command() {
        let mut filter = HandEyeCalibrationFilter::new(serde_json::json!({}));
        let cmd = ApiRequest::new("start", "", serde_json::Value::Null, "1");
        filter.process_command(&cmd);
        assert_eq!(*filter.state(), HandEyeCalibrationState::Collecting);
    }

    #[test]
    fn stop_command() {
        let mut filter = HandEyeCalibrationFilter::new(serde_json::json!({}));
        let start = ApiRequest::new("start", "", serde_json::Value::Null, "1");
        filter.process_command(&start);

        let stop = ApiRequest::new("stop", "", serde_json::Value::Null, "2");
        filter.process_command(&stop);
        assert_eq!(*filter.state(), HandEyeCalibrationState::Idle);
    }

    #[test]
    fn solve_fails_with_too_few_pairs() {
        let mut filter = HandEyeCalibrationFilter::new(serde_json::json!({}));
        let start = ApiRequest::new("start", "", serde_json::Value::Null, "1");
        filter.process_command(&start);

        let solve = ApiRequest::new("solve", "", serde_json::Value::Null, "2");
        filter.process_command(&solve);
        assert_eq!(*filter.state(), HandEyeCalibrationState::Failed);
    }

    #[test]
    fn reset_clears_state() {
        let mut filter = HandEyeCalibrationFilter::new(serde_json::json!({}));
        let start = ApiRequest::new("start", "", serde_json::Value::Null, "1");
        filter.process_command(&start);

        let reset = ApiRequest::new("reset", "", serde_json::Value::Null, "2");
        filter.process_command(&reset);
        assert_eq!(*filter.state(), HandEyeCalibrationState::Idle);
        assert_eq!(filter.result().n_pairs, 0);
    }

    #[test]
    fn ignores_data_when_idle() {
        let mut filter = HandEyeCalibrationFilter::new(serde_json::json!({
            "sourceAId": "a",
            "sourceBId": "b"
        }));

        let optical = OpticalData {
            sender_id: "a".into(),
            ..Default::default()
        };
        filter.process_data(StreamableData::Optical(optical));
        assert_eq!(filter.result().n_pairs, 0);
    }
}
