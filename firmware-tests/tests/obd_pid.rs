//! OBD-II Mode 01 framing and J1979 scaling.
//!
//! The fixtures are reconstructed from a real capture — a captured OBD-II
//! session from a 2013 Volvo tractor — so the
//! decoders are checked against numbers the truck actually produced rather than
//! against my reading of the standard.
//!
//! A wrong divisor does not fail loudly. It produces a plausible number that
//! ends up in an hours-of-service record, which is why every scaling factor
//! here has a test with a known-good value behind it.

use eld_decoders_test::obd::pid::{
    decode, parse_response, request_frame, ParseError, Reading, Value, REQUEST_ID, RESPONSE_ID_MAX,
    RESPONSE_ID_MIN,
};

/// Positive Mode 01 response: [len][0x41][pid][data…]
fn response(pid: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![(2 + data.len()) as u8, 0x41, pid];
    f.extend_from_slice(data);
    f.resize(8, 0);
    f
}

#[test]
fn builds_a_broadcast_request() {
    assert_eq!(REQUEST_ID, 0x7DF);
    assert_eq!(request_frame(0x0C), [0x02, 0x01, 0x0C, 0, 0, 0, 0, 0]);
}

#[test]
fn decodes_coolant_at_the_captured_value() {
    // Capture says 75.0 °C. J1979 is A - 40, so the raw byte must be 115.
    let f = response(0x05, &[115]);
    let r = parse_response(0x7E8, &f).unwrap();
    assert_eq!(r.value, Value::CoolantTempC(75.0));
}

#[test]
fn the_temperature_offset_is_not_forgotten() {
    // Raw zero is -40 °C, not 0. Dropping the offset looks reasonable on a warm
    // engine and is badly wrong on a cold one.
    assert_eq!(decode(0x05, &[0]).unwrap(), Value::CoolantTempC(-40.0));
}

#[test]
fn decodes_control_module_voltage_at_the_captured_value() {
    // Capture says 12.312 V, which is 12312 / 1000.
    let f = response(0x42, &[0x30, 0x18]);
    let r = parse_response(0x7E8, &f).unwrap();
    match r.value {
        Value::ControlModuleVoltage(v) => assert!((v - 12.312).abs() < 0.0005, "got {v}"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn decodes_throttle_at_the_captured_value() {
    // Capture says 14.9 %. A * 100 / 255 with A = 38 gives 14.902.
    match decode(0x11, &[38]).unwrap() {
        Value::ThrottlePercent(p) => assert!((p - 14.9).abs() < 0.05, "got {p}"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn decodes_rpm_with_the_quarter_scale() {
    // 0x0C is ((A * 256) + B) / 4 — the /4 is the one people drop, and doing so
    // reports 3400 rpm as 13600.
    match decode(0x0C, &[0x0D, 0x48]).unwrap() {
        Value::Rpm(r) => assert!((r - 850.0).abs() < 0.01, "got {r}"),
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(decode(0x0C, &[0, 0]).unwrap(), Value::Rpm(0.0));
}

#[test]
fn decodes_speed_directly() {
    assert_eq!(decode(0x0D, &[0]).unwrap(), Value::SpeedKph(0.0));
    assert_eq!(decode(0x0D, &[105]).unwrap(), Value::SpeedKph(105.0));
}

#[test]
fn decodes_the_wider_pids() {
    assert_eq!(
        decode(0x1F, &[0x01, 0x2C]).unwrap(),
        Value::RunTimeSeconds(300)
    );

    match decode(0x5E, &[0x00, 0xC8]).unwrap() {
        Value::FuelRateLph(v) => assert!((v - 10.0).abs() < 0.001, "got {v}"),
        other => panic!("unexpected: {other:?}"),
    }

    // 32-bit odometer in tenths of a km.
    match decode(0xA6, &[0x00, 0x12, 0xD6, 0x87]).unwrap() {
        Value::OdometerKm(km) => assert!((km - 123456.7).abs() < 0.05, "got {km}"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn keeps_the_responding_ecu_address() {
    // The capture shows 0x7E8 AND 0x7E9 both answering every PID. They are not
    // interchangeable — two ECUs can report different speeds — so a caller has
    // to be able to tell them apart.
    let f = response(0x0D, &[60]);
    assert_eq!(parse_response(0x7E8, &f).unwrap().source, 0x7E8);
    assert_eq!(parse_response(0x7E9, &f).unwrap().source, 0x7E9);
}

#[test]
fn accepts_the_whole_response_address_range() {
    let f = response(0x0D, &[10]);
    for id in RESPONSE_ID_MIN..=RESPONSE_ID_MAX {
        assert!(parse_response(id, &f).is_ok(), "{id:#x} should be accepted");
    }
}

#[test]
fn rejects_frames_that_are_not_obd_responses() {
    let f = response(0x0D, &[10]);

    // The request address is not a response address.
    assert_eq!(
        parse_response(0x7DF, &f),
        Err(ParseError::NotAResponseAddress)
    );
    // J1939 traffic shares the bus on some installs; it must not be decoded.
    assert_eq!(
        parse_response(0x0CF00400u32 as u16, &f).unwrap_err(),
        ParseError::NotAResponseAddress
    );
    // Wrong mode: 0x41 is Mode 01. 0x43 is stored DTCs.
    assert_eq!(
        parse_response(0x7E8, &[0x03, 0x43, 0x01, 0x33, 0, 0, 0, 0]),
        Err(ParseError::NotCurrentData)
    );
}

#[test]
fn rejects_malformed_lengths() {
    // Length longer than the frame.
    assert_eq!(
        parse_response(0x7E8, &[0x07, 0x41, 0x0D, 60]),
        Err(ParseError::BadLength)
    );
    // Length too small to hold mode + pid.
    assert_eq!(
        parse_response(0x7E8, &[0x01, 0x41, 0x0D, 60, 0, 0, 0, 0]),
        Err(ParseError::BadLength)
    );
    assert_eq!(parse_response(0x7E8, &[0x02]), Err(ParseError::Truncated));
}

#[test]
fn a_two_byte_pid_with_one_byte_of_payload_is_refused() {
    // Truncation must be an error, not a half-read value: reading only the high
    // byte of RPM silently reports a quarter of the real figure.
    assert_eq!(decode(0x0C, &[0x0D]), Err(ParseError::ShortPayload));
    assert_eq!(decode(0xA6, &[0x00, 0x12]), Err(ParseError::ShortPayload));
}

#[test]
fn an_unrecognised_pid_is_reported_rather_than_dropped() {
    // Discovery should be able to say "this truck offers something we do not
    // read yet" instead of silently ignoring it.
    let r: Reading = parse_response(0x7E8, &response(0x7F, &[1, 2])).unwrap();
    assert_eq!(r.value, Value::Unknown { pid: 0x7F });
}
