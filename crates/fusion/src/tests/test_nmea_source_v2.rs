use crate::sources::nmea_source::{
    ddmm_to_decimal, parse_gga, parse_nmea_sentence, verify_nmea_checksum, GpsType,
};
use fusion_types::GnssData;

/// Compute the NMEA checksum (XOR of all bytes between '$' and '*').
fn compute_nmea_checksum(sentence: &str) -> u8 {
    let start = sentence.find('$').unwrap() + 1;
    let end = sentence.find('*').unwrap();
    let mut checksum: u8 = 0;
    for b in sentence[start..end].bytes() {
        checksum ^= b;
    }
    checksum
}

#[test]
fn test_parse_gga_sentence() {
    let gga = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,*4F";
    let mut out = GnssData::default();
    let result = parse_nmea_sentence(gga, &mut out, GpsType::Single, false);

    assert_eq!(result, Some("GGA"));

    // Latitude: 4807.038 => 48 deg + 07.038 min = 48 + 7.038/60 = 48.1173
    let expected_lat = ddmm_to_decimal(4807.038);
    assert!(
        (out.latitude - expected_lat).abs() < 1e-6,
        "Latitude mismatch: expected {}, got {}",
        expected_lat,
        out.latitude
    );

    // Longitude: 01131.000 => 11 deg + 31.000 min = 11 + 31/60 = 11.51667
    let expected_lon = ddmm_to_decimal(1131.000);
    assert!(
        (out.longitude - expected_lon).abs() < 1e-6,
        "Longitude mismatch: expected {}, got {}",
        expected_lon,
        out.longitude
    );

    // Altitude: 545.4 M
    assert!(
        (out.altitude - 545.4).abs() < 1e-6,
        "Altitude mismatch: expected 545.4, got {}",
        out.altitude
    );

    // Quality: 1
    assert_eq!(out.quality, 1);

    // Number of satellites: 08
    assert_eq!(out.n_sat, 8);

    // HDOP: 0.9
    assert!(
        (out.hdop - 0.9).abs() < 1e-6,
        "HDOP mismatch: expected 0.9, got {}",
        out.hdop
    );
}

#[test]
fn test_parse_rmc_sentence() {
    // RMC is not directly parsed by the NMEA source (it parses GGA, GSA, VTG, HDT).
    // Verify that an RMC sentence is gracefully ignored (returns None).
    let rmc = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
    let mut out = GnssData::default();
    let result = parse_nmea_sentence(rmc, &mut out, GpsType::Single, false);

    // RMC is not handled by this parser, so it returns None
    assert_eq!(
        result, None,
        "RMC sentence should not be parsed by this NMEA source"
    );
}

#[test]
fn test_invalid_checksum() {
    // Take a valid GGA sentence and corrupt the checksum
    let bad_checksum = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,*00";
    assert!(
        !verify_nmea_checksum(bad_checksum.as_bytes()),
        "Bad checksum should be rejected"
    );

    // Also verify that parse_nmea_sentence rejects it
    let mut out = GnssData::default();
    let result = parse_nmea_sentence(bad_checksum, &mut out, GpsType::Single, false);
    assert_eq!(
        result, None,
        "Sentence with invalid checksum should not parse"
    );
}

#[test]
fn test_nmea_checksum_calculation() {
    // Verify correct checksum for a known GGA sentence
    let gga = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,*4F";
    let expected_checksum = 0x4Fu8;
    let computed = compute_nmea_checksum(gga);
    assert_eq!(
        computed, expected_checksum,
        "Checksum should be 0x4F, got 0x{:02X}",
        computed
    );

    // Verify the checksum validator agrees
    assert!(
        verify_nmea_checksum(gga.as_bytes()),
        "Valid GGA sentence should pass checksum verification"
    );

    // Verify a second known sentence
    let gga2 = "$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76";
    assert!(
        verify_nmea_checksum(gga2.as_bytes()),
        "Second GGA sentence should pass checksum verification"
    );
    let computed2 = compute_nmea_checksum(gga2);
    assert_eq!(
        computed2, 0x76,
        "Checksum should be 0x76, got 0x{:02X}",
        computed2
    );
}

#[test]
fn test_ddmm_to_decimal_north() {
    // 4807.038 => 48 degrees 07.038 minutes => 48 + 7.038/60
    let result = ddmm_to_decimal(4807.038);
    let expected = 48.0 + 7.038 / 60.0;
    assert!(
        (result - expected).abs() < 1e-8,
        "Expected {}, got {}",
        expected,
        result
    );
}

#[test]
fn test_ddmm_to_decimal_lon() {
    // 01131.000 => 11 degrees 31.000 minutes => 11 + 31/60
    let result = ddmm_to_decimal(1131.000);
    let expected = 11.0 + 31.0 / 60.0;
    assert!(
        (result - expected).abs() < 1e-8,
        "Expected {}, got {}",
        expected,
        result
    );
}

#[test]
fn test_missing_star_rejected() {
    // Sentence without '*' delimiter
    let bad = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,47";
    assert!(
        !verify_nmea_checksum(bad.as_bytes()),
        "Sentence without '*' should be rejected"
    );
}

#[test]
fn test_missing_dollar_rejected() {
    // Sentence without '$' prefix
    let bad = "GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,*47";
    assert!(
        !verify_nmea_checksum(bad.as_bytes()),
        "Sentence without '$' should be rejected"
    );
}

#[test]
fn test_gga_south_west_hemisphere() {
    // Construct a GGA sentence with S and W hemispheres using parse_gga directly.
    // Fields after the sentence ID: time, lat, N/S, lon, E/W, quality, nsat, hdop, alt, M, geoid, M, age, refid
    let fields = [
        "123519",    // UTC time
        "3352.0000", // Latitude (33 deg 52 min)
        "S",         // South
        "15112.0000", // Longitude (151 deg 12 min)
        "W",         // West
        "1",         // Quality
        "10",        // Satellites
        "1.0",       // HDOP
        "100.0",     // Altitude
        "M",
        "20.0", // Geoid separation
        "M",
        "1.5", // Diff age
        "",    // Reference station
    ];

    let mut out = GnssData::default();
    parse_gga(&fields, &mut out, false);

    assert!(
        out.latitude < 0.0,
        "Southern latitude should be negative, got {}",
        out.latitude
    );
    assert!(
        out.longitude < 0.0,
        "Western longitude should be negative, got {}",
        out.longitude
    );
    assert!(
        (out.altitude - 100.0).abs() < 1e-6,
        "Altitude mismatch: expected 100.0, got {}",
        out.altitude
    );
}
