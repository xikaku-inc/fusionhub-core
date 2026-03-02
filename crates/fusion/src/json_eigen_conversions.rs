use fusion_types::{Quatd, Trafo3d, Vec3d};
use nalgebra::{Translation3, UnitQuaternion};
use serde_json::Value;

/// Parse a Vec3d from JSON.
/// Accepts either `{"x": .., "y": .., "z": ..}` or `[x, y, z]`.
pub fn vec3d_from_json(v: &Value) -> Option<Vec3d> {
    if let Some(arr) = v.as_array() {
        if arr.len() >= 3 {
            let x = arr[0].as_f64()?;
            let y = arr[1].as_f64()?;
            let z = arr[2].as_f64()?;
            return Some(Vec3d::new(x, y, z));
        }
        return None;
    }

    if v.is_object() {
        let x = v.get("x").and_then(|v| v.as_f64())?;
        let y = v.get("y").and_then(|v| v.as_f64())?;
        let z = v.get("z").and_then(|v| v.as_f64())?;
        return Some(Vec3d::new(x, y, z));
    }

    None
}

/// Parse a UnitQuaternion from JSON.
/// Accepts either `{"w": .., "x": .., "y": .., "z": ..}` or `[w, x, y, z]`.
pub fn quatd_from_json(v: &Value) -> Option<Quatd> {
    if let Some(arr) = v.as_array() {
        if arr.len() >= 4 {
            let w = arr[0].as_f64()?;
            let x = arr[1].as_f64()?;
            let y = arr[2].as_f64()?;
            let z = arr[3].as_f64()?;
            let q = nalgebra::Quaternion::new(w, x, y, z);
            return Some(UnitQuaternion::from_quaternion(q));
        }
        return None;
    }

    if v.is_object() {
        let w = v.get("w").and_then(|v| v.as_f64())?;
        let x = v.get("x").and_then(|v| v.as_f64())?;
        let y = v.get("y").and_then(|v| v.as_f64())?;
        let z = v.get("z").and_then(|v| v.as_f64())?;
        let q = nalgebra::Quaternion::new(w, x, y, z);
        return Some(UnitQuaternion::from_quaternion(q));
    }

    None
}

/// Serialize a Vec3d to JSON as `{"x": .., "y": .., "z": ..}`.
pub fn vec3d_to_json(v: &Vec3d) -> Value {
    serde_json::json!({
        "x": v.x,
        "y": v.y,
        "z": v.z,
    })
}

/// Serialize a UnitQuaternion to JSON as `{"w": .., "x": .., "y": .., "z": ..}`.
pub fn quatd_to_json(q: &Quatd) -> Value {
    serde_json::json!({
        "w": q.w,
        "x": q.i,
        "y": q.j,
        "z": q.k,
    })
}

/// Parse a Trafo3d (Isometry3) from JSON.
///
/// Expects an object with `"translation"` and `"rotation"` fields:
/// ```json
/// {
///   "translation": {"x": 1.0, "y": 2.0, "z": 3.0},
///   "rotation": {"w": 1.0, "x": 0.0, "y": 0.0, "z": 0.0}
/// }
/// ```
///
/// Both fields can also be arrays:
/// ```json
/// {
///   "translation": [1.0, 2.0, 3.0],
///   "rotation": [1.0, 0.0, 0.0, 0.0]
/// }
/// ```
pub fn trafo3d_from_json(v: &Value) -> Option<Trafo3d> {
    let translation_val = v.get("translation")?;
    let rotation_val = v.get("rotation")?;

    let t = vec3d_from_json(translation_val)?;
    let r = quatd_from_json(rotation_val)?;

    Some(Trafo3d::from_parts(
        Translation3::new(t.x, t.y, t.z),
        r,
    ))
}

/// Serialize a Trafo3d (Isometry3) to JSON.
pub fn trafo3d_to_json(t: &Trafo3d) -> Value {
    let translation = t.translation.vector;
    let rotation = t.rotation;
    serde_json::json!({
        "translation": {
            "x": translation.x,
            "y": translation.y,
            "z": translation.z,
        },
        "rotation": {
            "w": rotation.w,
            "x": rotation.i,
            "y": rotation.j,
            "z": rotation.k,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use serde_json::json;

    #[test]
    fn vec3d_from_json_object() {
        let v = json!({"x": 1.0, "y": 2.0, "z": 3.0});
        let result = vec3d_from_json(&v).unwrap();
        assert_relative_eq!(result.x, 1.0);
        assert_relative_eq!(result.y, 2.0);
        assert_relative_eq!(result.z, 3.0);
    }

    #[test]
    fn vec3d_from_json_array() {
        let v = json!([4.0, 5.0, 6.0]);
        let result = vec3d_from_json(&v).unwrap();
        assert_relative_eq!(result.x, 4.0);
        assert_relative_eq!(result.y, 5.0);
        assert_relative_eq!(result.z, 6.0);
    }

    #[test]
    fn vec3d_from_json_invalid() {
        assert!(vec3d_from_json(&json!(42)).is_none());
        assert!(vec3d_from_json(&json!([1.0, 2.0])).is_none());
        assert!(vec3d_from_json(&json!({"x": 1.0})).is_none());
    }

    #[test]
    fn quatd_from_json_object() {
        let v = json!({"w": 1.0, "x": 0.0, "y": 0.0, "z": 0.0});
        let result = quatd_from_json(&v).unwrap();
        assert_relative_eq!(result.w, 1.0, epsilon = 1e-10);
    }

    #[test]
    fn quatd_from_json_array() {
        let v = json!([1.0, 0.0, 0.0, 0.0]);
        let result = quatd_from_json(&v).unwrap();
        assert_relative_eq!(result.w, 1.0, epsilon = 1e-10);
    }

    #[test]
    fn quatd_from_json_invalid() {
        assert!(quatd_from_json(&json!(42)).is_none());
        assert!(quatd_from_json(&json!([1.0, 0.0])).is_none());
    }

    #[test]
    fn vec3d_roundtrip() {
        let original = Vec3d::new(1.5, -2.3, 0.7);
        let json_val = vec3d_to_json(&original);
        let restored = vec3d_from_json(&json_val).unwrap();
        assert_relative_eq!(original.x, restored.x);
        assert_relative_eq!(original.y, restored.y);
        assert_relative_eq!(original.z, restored.z);
    }

    #[test]
    fn quatd_roundtrip() {
        let original = Quatd::identity();
        let json_val = quatd_to_json(&original);
        let restored = quatd_from_json(&json_val).unwrap();
        assert_relative_eq!(original.w, restored.w, epsilon = 1e-10);
        assert_relative_eq!(original.i, restored.i, epsilon = 1e-10);
        assert_relative_eq!(original.j, restored.j, epsilon = 1e-10);
        assert_relative_eq!(original.k, restored.k, epsilon = 1e-10);
    }

    #[test]
    fn trafo3d_from_json_objects() {
        let v = json!({
            "translation": {"x": 1.0, "y": 2.0, "z": 3.0},
            "rotation": {"w": 1.0, "x": 0.0, "y": 0.0, "z": 0.0}
        });
        let result = trafo3d_from_json(&v).unwrap();
        assert_relative_eq!(result.translation.x, 1.0);
        assert_relative_eq!(result.translation.y, 2.0);
        assert_relative_eq!(result.translation.z, 3.0);
        assert_relative_eq!(result.rotation.w, 1.0, epsilon = 1e-10);
    }

    #[test]
    fn trafo3d_from_json_arrays() {
        let v = json!({
            "translation": [1.0, 2.0, 3.0],
            "rotation": [1.0, 0.0, 0.0, 0.0]
        });
        let result = trafo3d_from_json(&v).unwrap();
        assert_relative_eq!(result.translation.x, 1.0);
    }

    #[test]
    fn trafo3d_roundtrip() {
        let original = Trafo3d::from_parts(
            Translation3::new(10.0, 20.0, 30.0),
            Quatd::identity(),
        );
        let json_val = trafo3d_to_json(&original);
        let restored = trafo3d_from_json(&json_val).unwrap();
        assert_relative_eq!(original.translation.x, restored.translation.x);
        assert_relative_eq!(original.translation.y, restored.translation.y);
        assert_relative_eq!(original.translation.z, restored.translation.z);
    }

    #[test]
    fn trafo3d_from_json_missing_fields() {
        assert!(trafo3d_from_json(&json!({"translation": [1,2,3]})).is_none());
        assert!(trafo3d_from_json(&json!({"rotation": [1,0,0,0]})).is_none());
        assert!(trafo3d_from_json(&json!(42)).is_none());
    }
}
