use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use crate::clock::{Clock, Clockwork};

/// Port of testClockTime.cpp
/// Creates a Clockwork, updates twice with 1-second intervals,
/// verifies elapsed duration is approximately 2 seconds within 20ms.
#[test]
fn clock_creation_and_update() {
    let mut cw = Clockwork::new();
    let dt = Duration::from_millis(1000);

    // September 1, 2024 00:00:00 UTC
    let t0 = UNIX_EPOCH + Duration::from_secs(1725148800);

    let clock = Clock::new("clock");
    cw.add_clock(clock);
    cw.update_clock("clock", t0);

    thread::sleep(dt);
    cw.update_clock("clock", t0 + dt);

    thread::sleep(dt);
    let t1 = cw.get_clock("clock").unwrap().now();

    let dt_measured = t1
        .duration_since(t0)
        .unwrap_or(Duration::ZERO)
        .as_millis() as i64;

    let expected = 2 * dt.as_millis() as i64;
    let error = (expected - dt_measured).abs();

    assert!(
        error < 100,
        "Expected ~{}ms elapsed, measured {}ms, error {}ms exceeds 100ms tolerance",
        expected,
        dt_measured,
        error
    );
}
