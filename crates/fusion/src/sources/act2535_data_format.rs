use fusion_types::VelocityMeterData;

/// Parse an ACT2535 CSV line into VelocityMeterData.
///
/// Expected format: "counter,velocity,distance,material,dopplerLevel,outputStatus"
///
/// Raw values are scaled:
/// - velocity: raw / 10.0 / 3600.0 (converts from 0.1 km/h to m/s)
/// - distance: raw / 10000.0 (converts from 0.1 mm to m)
/// - material: raw / 10.0 (converts from 0.1 V to V)
pub fn parse_act2535_line(line: &str) -> Option<VelocityMeterData> {
    let fields: Vec<&str> = line.split(',').map(|s| s.trim()).collect();

    if fields.len() != 6 {
        return None;
    }

    let counter = fields[0].parse::<i32>().ok()?;
    let velocity = fields[1].parse::<f64>().ok()? / 10.0 / 3600.0;
    let distance = fields[2].parse::<f64>().ok()? / 10000.0;
    let material = fields[3].parse::<f64>().ok()? / 10.0;
    let doppler_level = fields[4].parse::<f64>().ok()?;
    let output_status = fields[5].parse::<i32>().ok()?;

    let mut vmd = VelocityMeterData::default();
    vmd.counter = counter;
    vmd.velocity = velocity;
    vmd.distance = distance;
    vmd.material = material;
    vmd.doppler_level = doppler_level;
    vmd.output_status = output_status;

    Some(vmd)
}

/// Format VelocityMeterData back into an ACT2535 CSV line.
///
/// Reverses the scaling applied during parsing.
pub fn format_act2535_line(vmd: &VelocityMeterData) -> String {
    let raw_velocity = (vmd.velocity * 10.0 * 3600.0).round() as i64;
    let raw_distance = (vmd.distance * 10000.0).round() as i64;
    let raw_material = (vmd.material * 10.0).round() as i64;

    format!(
        "{}, {}, {}, {}, {:.2}, {}\r\n",
        vmd.counter, raw_velocity, raw_distance, raw_material, vmd.doppler_level, vmd.output_status
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_line() {
        let line = "42, 3600, 10000, 50, 12.34, 1";
        let vmd = parse_act2535_line(line).unwrap();
        assert_eq!(vmd.counter, 42);
        assert!((vmd.velocity - 0.1).abs() < 1e-9);
        assert!((vmd.distance - 1.0).abs() < 1e-9);
        assert!((vmd.material - 5.0).abs() < 1e-9);
        assert!((vmd.doppler_level - 12.34).abs() < 1e-9);
        assert_eq!(vmd.output_status, 1);
    }

    #[test]
    fn parse_wrong_field_count() {
        assert!(parse_act2535_line("1,2,3").is_none());
        assert!(parse_act2535_line("1,2,3,4,5,6,7").is_none());
    }

    #[test]
    fn parse_invalid_number() {
        assert!(parse_act2535_line("abc,2,3,4,5,6").is_none());
    }

    #[test]
    fn roundtrip() {
        let line = "42, 3600, 10000, 50, 12.34, 1";
        let vmd = parse_act2535_line(line).unwrap();
        let formatted = format_act2535_line(&vmd);
        let vmd2 = parse_act2535_line(&formatted).unwrap();
        assert_eq!(vmd.counter, vmd2.counter);
        assert!((vmd.velocity - vmd2.velocity).abs() < 1e-9);
        assert!((vmd.distance - vmd2.distance).abs() < 1e-9);
        assert!((vmd.material - vmd2.material).abs() < 1e-9);
        assert_eq!(vmd.output_status, vmd2.output_status);
    }
}
