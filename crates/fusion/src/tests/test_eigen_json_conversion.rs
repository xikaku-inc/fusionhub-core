use fusion_types::{Quatd, Trafo3d, Vec3d};
use nalgebra::{Isometry3, Quaternion, Translation3, UnitQuaternion, Vector3};

// ---------------------------------------------------------------------------
// JSON conversion helpers mirroring the C++ JsonEigenConversions
// These functions are what json_eigen_conversions.rs would expose. We test
// the serialization/deserialization patterns for Vec3d, Quatd, and Trafo3d.
// ---------------------------------------------------------------------------

fn vec3d_to_json(v: &Vec3d) -> serde_json::Value {
    serde_json::json!({"x": v.x, "y": v.y, "z": v.z})
}

fn vec3d_from_json(json: &serde_json::Value) -> Result<Vec3d, String> {
    if json.is_array() {
        let arr = json.as_array().ok_or("Expected array")?;
        if arr.len() != 3 {
            return Err(format!("Expected array of length 3, got {}", arr.len()));
        }
        Ok(Vec3d::new(
            arr[0].as_f64().ok_or("Expected f64")?,
            arr[1].as_f64().ok_or("Expected f64")?,
            arr[2].as_f64().ok_or("Expected f64")?,
        ))
    } else {
        Ok(Vec3d::new(
            json.get("x").and_then(|v| v.as_f64()).ok_or("Missing x")?,
            json.get("y").and_then(|v| v.as_f64()).ok_or("Missing y")?,
            json.get("z").and_then(|v| v.as_f64()).ok_or("Missing z")?,
        ))
    }
}

fn quatd_to_json(q: &Quatd) -> serde_json::Value {
    serde_json::json!({"w": q.w, "x": q.i, "y": q.j, "z": q.k})
}

fn quatd_from_json(json: &serde_json::Value) -> Result<Quatd, String> {
    if json.is_array() {
        let arr = json.as_array().ok_or("Expected array")?;
        if arr.len() != 4 {
            return Err(format!("Expected array of length 4, got {}", arr.len()));
        }
        let w = arr[0].as_f64().ok_or("Expected f64")?;
        let x = arr[1].as_f64().ok_or("Expected f64")?;
        let y = arr[2].as_f64().ok_or("Expected f64")?;
        let z = arr[3].as_f64().ok_or("Expected f64")?;
        Ok(UnitQuaternion::from_quaternion(Quaternion::new(w, x, y, z)))
    } else {
        let w = json.get("w").and_then(|v| v.as_f64()).ok_or("Missing w")?;
        let x = json.get("x").and_then(|v| v.as_f64()).ok_or("Missing x")?;
        let y = json.get("y").and_then(|v| v.as_f64()).ok_or("Missing y")?;
        let z = json.get("z").and_then(|v| v.as_f64()).ok_or("Missing z")?;
        Ok(UnitQuaternion::from_quaternion(Quaternion::new(w, x, y, z)))
    }
}

fn trafo3d_to_json(t: &Trafo3d) -> serde_json::Value {
    let trans = t.translation.vector;
    let quat = t.rotation;
    serde_json::json!({
        "vect": {"x": trans.x, "y": trans.y, "z": trans.z},
        "quat": {"w": quat.w, "x": quat.i, "y": quat.j, "z": quat.k}
    })
}

fn trafo3d_from_json(json: &serde_json::Value) -> Result<Trafo3d, String> {
    let vect = json.get("vect").ok_or("Missing vect")?;
    let quat_json = json.get("quat").ok_or("Missing quat")?;
    let v = vec3d_from_json(vect)?;
    let q = quatd_from_json(quat_json)?;
    Ok(Isometry3::from_parts(Translation3::from(v), q))
}

// ---------------------------------------------------------------------------
// Port of testEigenJsonConversion.cpp
// ---------------------------------------------------------------------------

/// Port of testEigenJsonConversion.cpp - Vec3d object roundtrip
#[test]
fn vec3d_object_roundtrip() {
    let vec = Vec3d::new(1.0, 2.0, 3.0);
    let json = vec3d_to_json(&vec);

    assert_eq!(json["x"], 1.0);
    assert_eq!(json["y"], 2.0);
    assert_eq!(json["z"], 3.0);

    let deserialized = vec3d_from_json(&json).unwrap();
    assert_eq!(deserialized, vec);
}

/// Port of testEigenJsonConversion.cpp - Vec3d from array
#[test]
fn vec3d_from_array() {
    let json: serde_json::Value = serde_json::from_str("[1.0,2.0,3.0]").unwrap();
    let vec = vec3d_from_json(&json).unwrap();
    assert_eq!(vec.x, 1.0);
    assert_eq!(vec.y, 2.0);
    assert_eq!(vec.z, 3.0);
}

/// Port of testEigenJsonConversion.cpp - Quatd object roundtrip
#[test]
fn quatd_object_roundtrip() {
    let quat = UnitQuaternion::from_quaternion(Quaternion::new(0.5, 0.3, 0.2, 0.1));
    let json = quatd_to_json(&quat);

    // Note: nalgebra normalizes the quaternion, so we compare the normalized values
    assert!((json["w"].as_f64().unwrap() - quat.w).abs() < 1e-10);
    assert!((json["x"].as_f64().unwrap() - quat.i).abs() < 1e-10);
    assert!((json["y"].as_f64().unwrap() - quat.j).abs() < 1e-10);
    assert!((json["z"].as_f64().unwrap() - quat.k).abs() < 1e-10);

    let deserialized = quatd_from_json(&json).unwrap();
    assert!(
        deserialized.angle_to(&quat) < 1e-10,
        "Quaternion roundtrip mismatch"
    );
}

/// Port of testEigenJsonConversion.cpp - Quatd from array
#[test]
fn quatd_from_array() {
    let json: serde_json::Value = serde_json::from_str("[1.0,2.0,3.0,4.0]").unwrap();
    let quat = quatd_from_json(&json).unwrap();
    // The quaternion is constructed from (w, x, y, z) = (1, 2, 3, 4) then normalized
    let expected = UnitQuaternion::from_quaternion(Quaternion::new(1.0, 2.0, 3.0, 4.0));
    assert!(quat.angle_to(&expected) < 1e-10);
}

/// Port of testEigenJsonConversion.cpp - Length validation for Vec3d
#[test]
fn vec3d_length_validation() {
    let mut arr = serde_json::Value::Array(Vec::new());

    // Arrays of wrong length should fail
    for i in 0..10 {
        let result = vec3d_from_json(&arr);
        if i == 3 {
            assert!(result.is_ok(), "Vec3d should parse from array of length 3");
        } else {
            assert!(
                result.is_err(),
                "Vec3d should reject array of length {}",
                i
            );
        }
        arr.as_array_mut().unwrap().push(serde_json::json!(i));
    }
}

/// Port of testEigenJsonConversion.cpp - Length validation for Quatd
#[test]
fn quatd_length_validation() {
    let mut arr = serde_json::Value::Array(Vec::new());

    for i in 0..10 {
        let result = quatd_from_json(&arr);
        if i == 4 {
            assert!(
                result.is_ok(),
                "Quatd should parse from array of length 4"
            );
        } else {
            assert!(
                result.is_err(),
                "Quatd should reject array of length {}",
                i
            );
        }
        arr.as_array_mut().unwrap().push(serde_json::json!(i));
    }
}

/// Port of testEigenJsonConversion.cpp - Trafo3d roundtrip
#[test]
fn trafo3d_roundtrip() {
    let rotation =
        UnitQuaternion::from_axis_angle(&Vector3::x_axis(), 0.5);
    let translation = Translation3::new(1.0, 2.0, 3.0);
    let trafo = Isometry3::from_parts(translation, rotation);

    let json = trafo3d_to_json(&trafo);

    // Verify expected quaternion values
    assert!((json["quat"]["w"].as_f64().unwrap() - rotation.w).abs() < 1e-10);
    assert!((json["quat"]["x"].as_f64().unwrap() - rotation.i).abs() < 1e-10);
    assert!((json["quat"]["y"].as_f64().unwrap() - rotation.j).abs() < 1e-10);
    assert!((json["quat"]["z"].as_f64().unwrap() - rotation.k).abs() < 1e-10);

    // Verify translation
    assert_eq!(json["vect"]["x"], 1.0);
    assert_eq!(json["vect"]["y"], 2.0);
    assert_eq!(json["vect"]["z"], 3.0);

    // Roundtrip
    let deserialized = trafo3d_from_json(&json).unwrap();
    assert!(
        (deserialized.translation.vector - trafo.translation.vector).norm() < 1e-10,
        "Translation mismatch"
    );
    assert!(
        deserialized.rotation.angle_to(&trafo.rotation) < 1e-10,
        "Rotation mismatch"
    );
}
