use crate::sinks::nmea_sink::{
    checksum, decimal_degrees_to_nmea, format_gga, format_hdt, format_rmc, format_vtg,
};
use std::time::SystemTime;

/// Reverse of decimal_degrees_to_nmea: convert DDMM.MMMMMM back to decimal degrees
fn from_nmea_coord(nmea_str: &str) -> f64 {
    let val: f64 = nmea_str.parse().unwrap_or(0.0);
    let d = (val / 100.0).floor();
    let m = val - d * 100.0;
    d + m / 60.0
}

/// Simple NMEA sentence parser for testing: validates structure + checksum, extracts fields
struct NmeaParsed {
    address: String,
    fields: Vec<String>,
    valid: bool,
}

fn parse_nmea(sentence: &str) -> NmeaParsed {
    let mut result = NmeaParsed {
        address: String::new(),
        fields: Vec::new(),
        valid: false,
    };

    // Strip trailing \r\n or literal \r\n
    let sentence = sentence
        .trim_end_matches("\\r\\n")
        .trim_end_matches("\r\n");

    if sentence.len() < 6 || !sentence.starts_with('$') {
        return result;
    }

    let star_pos = match sentence.find('*') {
        Some(pos) => pos,
        None => return result,
    };

    if star_pos + 2 >= sentence.len() {
        return result;
    }

    // Extract payload between '$' and '*'
    let payload = &sentence[1..star_pos];

    // Validate checksum
    let calculated: u8 = payload.bytes().fold(0u8, |acc, b| acc ^ b);
    let expected = &sentence[star_pos + 1..star_pos + 3];
    let calculated_hex = format!("{:02X}", calculated);
    if expected != calculated_hex {
        return result;
    }

    // Split by comma
    let parts: Vec<&str> = payload.split(',').collect();
    if parts.is_empty() {
        return result;
    }

    result.address = parts[0].to_string();
    result.fields = parts[1..].iter().map(|s| s.to_string()).collect();
    result.valid = true;
    result
}

// ---------------------------------------------------------------------------
// Port of testNmeaSink.cpp
// ---------------------------------------------------------------------------

/// Port of testNmeaSink.cpp - Checksum format (2 hex digits)
#[test]
fn checksum_format() {
    let cs = checksum("$GPGGA,test*");
    assert_eq!(cs.len(), 2);
    for c in cs.chars() {
        assert!(
            c.is_ascii_hexdigit() && (c.is_ascii_digit() || c.is_ascii_uppercase()),
            "Checksum char '{}' is not uppercase hex",
            c
        );
    }
}

/// Port of testNmeaSink.cpp - Coordinate conversion roundtrip: positive latitude
#[test]
fn coordinate_roundtrip_positive_latitude() {
    let lat = 48.117222;
    let (nmea_str, hem) = decimal_degrees_to_nmea(lat, false);
    assert_eq!(hem, 'N');
    let back = from_nmea_coord(&nmea_str);
    assert!(
        (back - lat).abs() < 1e-4,
        "Expected {}, got {}",
        lat,
        back
    );
}

/// Port of testNmeaSink.cpp - Coordinate conversion roundtrip: positive longitude
#[test]
fn coordinate_roundtrip_positive_longitude() {
    let lon = 11.516667;
    let (nmea_str, hem) = decimal_degrees_to_nmea(lon, true);
    assert_eq!(hem, 'E');
    let back = from_nmea_coord(&nmea_str);
    assert!(
        (back - lon).abs() < 1e-4,
        "Expected {}, got {}",
        lon,
        back
    );
}

/// Port of testNmeaSink.cpp - Coordinate conversion roundtrip: small coordinate
#[test]
fn coordinate_roundtrip_small_coordinate() {
    let coord = 1.5;
    let (nmea_str, _hem) = decimal_degrees_to_nmea(coord, false);
    let back = from_nmea_coord(&nmea_str);
    assert!(
        (back - coord).abs() < 1e-4,
        "Expected {}, got {}",
        coord,
        back
    );
}

/// Port of testNmeaSink.cpp - Coordinate conversion roundtrip: zero
#[test]
fn coordinate_roundtrip_zero() {
    let (nmea_str, _hem) = decimal_degrees_to_nmea(0.0, false);
    let back = from_nmea_coord(&nmea_str);
    assert!(back.abs() < 1e-6, "Expected 0.0, got {}", back);
}

/// Port of testNmeaSink.cpp - Coordinate conversion roundtrip: negative coordinate
#[test]
fn coordinate_roundtrip_negative() {
    let coord = -33.8688;
    let (nmea_str, hem) = decimal_degrees_to_nmea(coord, false);
    assert_eq!(hem, 'S');
    let back = from_nmea_coord(&nmea_str);
    assert!(
        (back - coord.abs()).abs() < 1e-4,
        "Expected {}, got {}",
        coord.abs(),
        back
    );
}

/// Port of testNmeaSink.cpp - GGA sentence generation contains GPGGA
#[test]
fn gga_sentence_contains_gpgga() {
    let sentence = format_gga(48.1173, 11.516, 520.0, 1, 10, 0.9);
    assert!(
        sentence.contains("GPGGA"),
        "GGA sentence does not contain GPGGA: {}",
        sentence
    );
}

/// Port of testNmeaSink.cpp - GGA sentence structure
#[test]
fn gga_sentence_structure() {
    let sentence = format_gga(48.0, 11.0, 500.0, 1, 10, 1.0);
    assert!(
        sentence.starts_with('$'),
        "Sentence should start with $: {}",
        sentence
    );
    assert!(
        sentence.contains('*'),
        "Sentence should contain *: {}",
        sentence
    );
    assert!(
        sentence.ends_with("\\r\\n"),
        "Sentence should end with \\r\\n: {}",
        sentence
    );
}

/// Port of testNmeaSink.cpp - RMC sentence generation
#[test]
fn rmc_sentence_contains_gprmc() {
    let sentence = format_rmc(48.0, 11.0, SystemTime::now(), 5.0, 180.0);
    assert!(
        sentence.contains("GPRMC"),
        "RMC sentence does not contain GPRMC: {}",
        sentence
    );
}

/// Port of testNmeaSink.cpp - VTG sentence generation
#[test]
fn vtg_sentence_contains_gpvtg() {
    let sentence = format_vtg(90.0, 10.0);
    assert!(
        sentence.contains("GPVTG"),
        "VTG sentence does not contain GPVTG: {}",
        sentence
    );
}

/// Port of testNmeaSink.cpp - HDT sentence generation
#[test]
fn hdt_sentence_contains_gphdt() {
    let sentence = format_hdt(270.0);
    assert!(
        sentence.contains("GPHDT"),
        "HDT sentence does not contain GPHDT: {}",
        sentence
    );
}

/// Port of testNmeaSink.cpp - HDT typical heading with checksum validation
#[test]
fn hdt_typical_heading_parseable() {
    let sentence = format_hdt(123.4);
    let parsed = parse_nmea(&sentence);
    assert!(parsed.valid, "HDT sentence failed checksum: {}", sentence);
    assert_eq!(parsed.address, "GPHDT");
    let heading: f64 = parsed.fields[0].parse().unwrap();
    assert!(
        (heading - 123.4).abs() < 0.2,
        "Expected ~123.4, got {}",
        heading
    );
}

/// Port of testNmeaSink.cpp - HDT zero heading
#[test]
fn hdt_zero_heading() {
    let sentence = format_hdt(0.0);
    let parsed = parse_nmea(&sentence);
    assert!(parsed.valid, "HDT sentence failed checksum: {}", sentence);
    let heading: f64 = parsed.fields[0].parse().unwrap();
    assert!((heading - 0.0).abs() < 0.2, "Expected ~0.0, got {}", heading);
}

/// Port of testNmeaSink.cpp - HDT heading near 360
#[test]
fn hdt_heading_near_360() {
    let sentence = format_hdt(359.9);
    let parsed = parse_nmea(&sentence);
    assert!(parsed.valid, "HDT sentence failed checksum: {}", sentence);
    let heading: f64 = parsed.fields[0].parse().unwrap();
    assert!(
        (heading - 359.9).abs() < 0.2,
        "Expected ~359.9, got {}",
        heading
    );
}

/// Port of testNmeaSink.cpp - Heading normalization: negative heading
#[test]
fn heading_normalization_negative() {
    let sentence = format_hdt(-90.0);
    assert!(
        sentence.contains("270.0"),
        "Expected 270.0 in sentence: {}",
        sentence
    );
}

/// Port of testNmeaSink.cpp - Heading normalization: > 360
#[test]
fn heading_normalization_over_360() {
    let sentence = format_hdt(450.0);
    assert!(
        sentence.contains("90.0"),
        "Expected 90.0 in sentence: {}",
        sentence
    );
}

/// Port of testNmeaSink.cpp - VTG parseable with correct structure
#[test]
fn vtg_parseable() {
    let sentence = format_vtg(54.7, 10.0);
    let parsed = parse_nmea(&sentence);
    assert!(parsed.valid, "VTG sentence failed checksum: {}", sentence);
    assert_eq!(parsed.address, "GPVTG");
    assert!(parsed.fields.len() >= 7);

    let track: f64 = parsed.fields[0].parse().unwrap();
    assert!((track - 54.7).abs() < 0.2, "Expected ~54.7, got {}", track);
    assert_eq!(parsed.fields[1], "T");
}

/// Port of testNmeaSink.cpp - GGA parseable with checksum
#[test]
fn gga_parseable_with_checksum() {
    let sentence = format_gga(48.117222, 11.516667, 545.4, 1, 8, 0.9);
    let parsed = parse_nmea(&sentence);
    assert!(parsed.valid, "GGA sentence failed checksum: {}", sentence);
    assert_eq!(parsed.address, "GPGGA");
    assert!(parsed.fields.len() >= 13);
}

/// Port of testNmeaSink.cpp - RMC parseable
#[test]
fn rmc_parseable() {
    let sentence = format_rmc(48.117222, 11.516667, SystemTime::now(), 5.0, 180.0);
    let parsed = parse_nmea(&sentence);
    assert!(parsed.valid, "RMC sentence failed checksum: {}", sentence);
    assert_eq!(parsed.address, "GPRMC");
    assert!(parsed.fields.len() >= 10);
}
