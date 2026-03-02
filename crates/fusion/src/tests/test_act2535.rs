use crate::sources::act2535_data_format::{format_act2535_line, parse_act2535_line};
use fusion_types::VelocityMeterData;

/// Port of testAct2535.cpp - Parse valid line
#[test]
fn parse_valid_line() {
    let result = parse_act2535_line("42, 5000, 12345, 30, 85.50, 1");
    assert!(result.is_some());

    let vmd = result.unwrap();
    assert_eq!(vmd.counter, 42);
    assert!((vmd.velocity - 5000.0 / 10.0 / 3600.0).abs() < 1e-9);
    assert!((vmd.distance - 12345.0 / 10000.0).abs() < 1e-9);
    assert!((vmd.material - 30.0 / 10.0).abs() < 1e-9);
    assert!((vmd.doppler_level - 85.50).abs() < 1e-9);
    assert_eq!(vmd.output_status, 1);
}

/// Port of testAct2535.cpp - Parse invalid lines: too few fields
#[test]
fn parse_too_few_fields() {
    let result = parse_act2535_line("42, 5000, 12345");
    assert!(result.is_none());
}

/// Port of testAct2535.cpp - Parse invalid lines: too many fields
#[test]
fn parse_too_many_fields() {
    let result = parse_act2535_line("42, 5000, 12345, 30, 85.50, 1, 99");
    assert!(result.is_none());
}

/// Port of testAct2535.cpp - Parse invalid lines: empty string
#[test]
fn parse_empty_string() {
    let result = parse_act2535_line("");
    assert!(result.is_none());
}

/// Port of testAct2535.cpp - Parse invalid lines: non-numeric data
#[test]
fn parse_non_numeric_data() {
    let result = parse_act2535_line("abc, def, ghi, jkl, mno, pqr");
    assert!(result.is_none());
}

/// Port of testAct2535.cpp - Format known data then parse roundtrip
#[test]
fn format_then_parse_roundtrip() {
    let mut vmd = VelocityMeterData::default();
    vmd.counter = 100;
    vmd.velocity = 5000.0 / 10.0 / 3600.0;
    vmd.distance = 12345.0 / 10000.0;
    vmd.material = 30.0 / 10.0;
    vmd.doppler_level = 85.50;
    vmd.output_status = 3;

    let line = format_act2535_line(&vmd);
    let parsed = parse_act2535_line(&line);
    assert!(parsed.is_some());

    let p = parsed.unwrap();
    assert_eq!(p.counter, 100);
    assert!((p.velocity - vmd.velocity).abs() < 1e-6);
    assert!((p.distance - vmd.distance).abs() < 1e-6);
    assert!((p.material - vmd.material).abs() < 1e-6);
    assert!((p.doppler_level - 85.50).abs() < 1e-9);
    assert_eq!(p.output_status, 3);
}

/// Port of testAct2535.cpp - Roundtrip loopback: typical values
#[test]
fn roundtrip_typical_values() {
    let mut input = VelocityMeterData::default();
    input.counter = 1;
    input.velocity = 5000.0 / 10.0 / 3600.0;
    input.distance = 25000.0 / 10000.0;
    input.material = 45.0 / 10.0;
    input.doppler_level = 92.30;
    input.output_status = 1;

    let wire = format_act2535_line(&input);
    let output = parse_act2535_line(&wire);
    assert!(output.is_some());

    let o = output.unwrap();
    assert_eq!(o.counter, input.counter);
    assert!((o.velocity - input.velocity).abs() < 1e-6);
    assert!((o.distance - input.distance).abs() < 1e-6);
    assert!((o.material - input.material).abs() < 1e-6);
    assert!((o.doppler_level - input.doppler_level).abs() < 1e-9);
    assert_eq!(o.output_status, input.output_status);
}

/// Port of testAct2535.cpp - Roundtrip loopback: zero velocity
#[test]
fn roundtrip_zero_velocity() {
    let mut input = VelocityMeterData::default();
    input.counter = 0;
    input.velocity = 0.0;
    input.distance = 0.0;
    input.material = 0.0;
    input.doppler_level = 0.0;
    input.output_status = 0;

    let wire = format_act2535_line(&input);
    let output = parse_act2535_line(&wire);
    assert!(output.is_some());

    let o = output.unwrap();
    assert_eq!(o.counter, 0);
    assert!((o.velocity - 0.0).abs() < 1e-9);
    assert!((o.distance - 0.0).abs() < 1e-9);
    assert!((o.material - 0.0).abs() < 1e-9);
    assert!((o.doppler_level - 0.0).abs() < 1e-9);
    assert_eq!(o.output_status, 0);
}

/// Port of testAct2535.cpp - Roundtrip loopback: large counter and high speed
#[test]
fn roundtrip_large_counter() {
    let mut input = VelocityMeterData::default();
    input.counter = 99999;
    input.velocity = 99999.0 / 10.0 / 3600.0;
    input.distance = 999999.0 / 10000.0;
    input.material = 100.0 / 10.0;
    input.doppler_level = 120.00;
    input.output_status = 3;

    let wire = format_act2535_line(&input);
    let output = parse_act2535_line(&wire);
    assert!(output.is_some());

    let o = output.unwrap();
    assert_eq!(o.counter, input.counter);
    assert!((o.velocity - input.velocity).abs() < 1e-6);
    assert!((o.distance - input.distance).abs() < 1e-6);
    assert!((o.material - input.material).abs() < 1e-6);
    assert!((o.doppler_level - input.doppler_level).abs() < 1e-9);
    assert_eq!(o.output_status, input.output_status);
}
