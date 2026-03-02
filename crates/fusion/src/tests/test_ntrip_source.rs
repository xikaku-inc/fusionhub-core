/// BCC checksum comparison for GGA sentences.
/// Computes the XOR of all characters between '$' and '*', then compares
/// with the two-character hex value after '*'.
/// Returns 0 on match, 1 on structural error, or the signed difference
/// between computed and expected checksum.
fn bcc_checksum_compare_for_gga(src: &str) -> i32 {
    let dollar_pos = match src.find('$') {
        Some(pos) => pos,
        None => return 1,
    };

    let star_pos = match src.find('*') {
        Some(pos) => pos,
        None => return 1,
    };

    let payload = &src[dollar_pos + 1..star_pos];

    let computed: u8 = payload.bytes().fold(0u8, |acc, b| acc ^ b);

    let expected_str = &src[star_pos + 1..];
    let expected = match u8::from_str_radix(expected_str.trim(), 16) {
        Ok(v) => v,
        Err(_) => return 1,
    };

    computed as i32 - expected as i32
}

// ---------------------------------------------------------------------------
// Port of testNTRIPSource.cpp
// ---------------------------------------------------------------------------

/// Port of testNTRIPSource.cpp - BCC checksum: empty payload
#[test]
fn bcc_checksum_empty_payload() {
    assert_eq!(bcc_checksum_compare_for_gga("$*0"), 0);
}

/// Port of testNTRIPSource.cpp - BCC checksum: single char
#[test]
fn bcc_checksum_single_char() {
    // 'A' XOR = 0x41
    assert_eq!(bcc_checksum_compare_for_gga("$A*41"), 0);
}

/// Port of testNTRIPSource.cpp - BCC checksum: two chars (XOR)
#[test]
fn bcc_checksum_two_chars_xor() {
    // 'A' ^ 'B' = 0x41 ^ 0x42 = 0x03
    // Expected in string is "41", so difference = 0x03 - 0x41 = 3 - 65 = -62
    // But C++ test: ('A' ^ 'B') - 0x41 = 3 - 65 = -62? No:
    // C++ test says BccCheckSumCompareForGGA("$AB*41") == ('A' ^ 'B') - 0x41
    // = 0x03 - 0x41 = -62
    let result = bcc_checksum_compare_for_gga("$AB*41");
    let expected = ('A' as u8 ^ 'B' as u8) as i32 - 0x41_i32;
    assert_eq!(result, expected);
}

/// Port of testNTRIPSource.cpp - BCC checksum: missing $ returns 1
#[test]
fn bcc_checksum_missing_dollar() {
    assert_eq!(bcc_checksum_compare_for_gga("A*41"), 1);
}

/// Port of testNTRIPSource.cpp - BCC checksum: missing * returns 1
#[test]
fn bcc_checksum_missing_star() {
    assert_eq!(bcc_checksum_compare_for_gga("$A41"), 1);
}
