//! UTC clock + NMEA date parsing.
//!
//! These guard the regulatory timestamp path. 49 CFR 395 Appendix A requires
//! ELD event times within ten minutes of UTC; before this work the firmware
//! carried a complete `UtcClock` that nothing ever instantiated, and every
//! record was stamped with uptime since boot.

use eld_decoders_test::nmea::{self, Nmea};
use eld_decoders_test::time::{
    ClockStatus, ClockSyncError, RecordStamp, UtcClock, UtcDateTime, MAX_UTC_HOLDOVER_MS,
};

/// Append the correct NMEA checksum so tests state the payload, not the hash.
fn sentence(payload: &str) -> String {
    let cs = payload.bytes().fold(0u8, |acc, b| acc ^ b);
    format!("${payload}*{cs:02X}")
}

const JAN_5_2026_UTC_MS: u64 = 1_767_571_200_000; // 2026-01-05T00:00:00Z

#[test]
fn utc_datetime_converts_a_known_instant() {
    let dt = UtcDateTime {
        year: 2026,
        month: 1,
        day: 5,
        hour: 0,
        minute: 0,
        second: 0,
        millis: 0,
    };
    assert_eq!(dt.to_unix_ms(), Some(JAN_5_2026_UTC_MS));
}

#[test]
fn utc_datetime_accepts_a_leap_day_and_rejects_a_false_one() {
    let leap = UtcDateTime {
        year: 2028,
        month: 2,
        day: 29,
        hour: 12,
        minute: 0,
        second: 0,
        millis: 0,
    };
    assert!(leap.to_unix_ms().is_some());

    let not_leap = UtcDateTime { year: 2026, ..leap };
    assert_eq!(not_leap.to_unix_ms(), None);
}

#[test]
fn utc_datetime_rejects_a_receiver_default_before_2020() {
    // A GPS module with no almanac often reports 1980 or 2000. Anchoring the
    // clock to that would backdate every duty record.
    let stale = UtcDateTime {
        year: 2000,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        millis: 0,
    };
    assert_eq!(stale.to_unix_ms(), None);
}

#[test]
fn clock_is_unsynchronized_until_it_is_told_the_time() {
    let clock = UtcClock::new();
    let reading = clock.reading(5_000);
    assert_eq!(reading.status, ClockStatus::Unsynchronized);
    assert_eq!(reading.utc_ms, None);
}

#[test]
fn clock_reports_utc_after_a_valid_sync() {
    let mut clock = UtcClock::new();
    clock.synchronize(JAN_5_2026_UTC_MS, 1_000).unwrap();

    let at_sync = clock.reading(1_000);
    assert_eq!(at_sync.status, ClockStatus::GpsSynchronized);
    assert_eq!(at_sync.utc_ms, Some(JAN_5_2026_UTC_MS));

    // Five minutes later, still inside the tolerance: holdover, still usable.
    let later = clock.reading(1_000 + 5 * 60_000);
    assert_eq!(later.status, ClockStatus::Holdover);
    assert_eq!(later.utc_ms, Some(JAN_5_2026_UTC_MS + 5 * 60_000));
}

#[test]
fn clock_maps_an_observation_before_the_first_sync_to_utc() {
    let mut clock = UtcClock::new();
    clock.synchronize(JAN_5_2026_UTC_MS, 10_000).unwrap();
    assert_eq!(clock.utc_at(8_500, 10_000), Some(JAN_5_2026_UTC_MS - 1_500));
}

#[test]
fn clock_refuses_event_conversion_after_holdover_expires() {
    let mut clock = UtcClock::new();
    clock.synchronize(JAN_5_2026_UTC_MS, 0).unwrap();
    assert_eq!(clock.utc_at(1_000, MAX_UTC_HOLDOVER_MS + 1), None);
}

#[test]
fn clock_stops_claiming_utc_once_holdover_exceeds_the_tolerance() {
    let mut clock = UtcClock::new();
    clock.synchronize(JAN_5_2026_UTC_MS, 0).unwrap();

    let stale = clock.reading(MAX_UTC_HOLDOVER_MS + 1);
    // Past ten minutes without a fix the device cannot stand behind the
    // timestamp, so it reports no UTC rather than an out-of-tolerance one.
    assert_eq!(stale.status, ClockStatus::Unsynchronized);
    assert_eq!(stale.utc_ms, None);
}

#[test]
fn clock_rejects_an_implausible_correction_once_running() {
    let mut clock = UtcClock::new();
    clock.synchronize(JAN_5_2026_UTC_MS, 0).unwrap();

    // A jump of an hour is a bad fix, not a real correction. Accepting it would
    // silently rewrite when the driver was driving.
    let err = clock.synchronize(JAN_5_2026_UTC_MS + 3_600_000, 1_000);
    assert_eq!(err, Err(ClockSyncError::ImplausibleCorrection));
    assert_eq!(clock.reading(1_000).utc_ms, Some(JAN_5_2026_UTC_MS + 1_000));
}

#[test]
fn clock_rejects_monotonic_time_moving_backwards() {
    let mut clock = UtcClock::new();
    clock.synchronize(JAN_5_2026_UTC_MS, 10_000).unwrap();
    assert_eq!(
        clock.synchronize(JAN_5_2026_UTC_MS, 9_000),
        Err(ClockSyncError::MonotonicWentBackwards)
    );
}

#[test]
fn clock_rejects_an_out_of_range_utc() {
    let mut clock = UtcClock::new();
    assert_eq!(clock.synchronize(0, 0), Err(ClockSyncError::InvalidUtc));
}

#[test]
fn record_stamp_distinguishes_uptime_from_wall_clock() {
    let synced = RecordStamp {
        monotonic_ms: 42,
        utc_ms: Some(JAN_5_2026_UTC_MS),
    };
    let prefix = synced.prefix();
    assert!(prefix.starts_with(&format!("{JAN_5_2026_UTC_MS} ")));
    assert!(prefix.contains("clk=utc"));
    assert!(prefix.contains("up=42"));

    // Without a fix the record must not look like it has a real time.
    let unsynced = RecordStamp::unsynchronized(42);
    assert!(unsynced.prefix().contains("clk=unsync"));
    assert_eq!(unsynced.utc_ms, None);
}

#[test]
fn rmc_supplies_the_date_that_gga_cannot() {
    let line = sentence("GPRMC,120000.000,A,4152.6300,N,08739.4200,W,0.0,0.0,050126,,,A");
    let Some(Nmea::Rmc {
        valid,
        datetime,
        lat,
        lon,
    }) = nmea::parse(&line)
    else {
        panic!("expected an RMC sentence");
    };

    assert!(valid);
    let dt = datetime.expect("RMC carries a date");
    assert_eq!((dt.year, dt.month, dt.day), (2026, 1, 5));
    assert_eq!((dt.hour, dt.minute, dt.second), (12, 0, 0));
    assert_eq!(dt.to_unix_ms(), Some(JAN_5_2026_UTC_MS + 12 * 3_600_000));

    // Position is carried, not discarded: every Appendix A event needs it.
    assert!((lat.unwrap() - 41.877_166).abs() < 1e-4);
    assert!((lon.unwrap() + 87.657).abs() < 1e-3);
}

#[test]
fn a_void_rmc_is_marked_invalid_so_it_cannot_anchor_the_clock() {
    // "V" means the receiver does not vouch for this fix, but the date field
    // still looks perfectly plausible.
    let line = sentence("GPRMC,120000.000,V,4152.6300,N,08739.4200,W,0.0,0.0,050126,,,N");
    let Some(Nmea::Rmc {
        valid, datetime, ..
    }) = nmea::parse(&line)
    else {
        panic!("expected an RMC sentence");
    };
    assert!(!valid);
    assert!(datetime.is_some());
}

#[test]
fn rmc_parses_fractional_seconds_as_milliseconds() {
    let line = sentence("GPRMC,120000.5,A,4152.6300,N,08739.4200,W,0.0,0.0,050126,,,A");
    let Some(Nmea::Rmc { datetime, .. }) = nmea::parse(&line) else {
        panic!("expected an RMC sentence");
    };
    // ".5" is half a second, not five milliseconds.
    assert_eq!(datetime.unwrap().millis, 500);
}

#[test]
fn rmc_with_a_missing_date_yields_no_datetime() {
    let line = sentence("GPRMC,120000.000,A,4152.6300,N,08739.4200,W,0.0,0.0,,,,A");
    let Some(Nmea::Rmc { datetime, .. }) = nmea::parse(&line) else {
        panic!("expected an RMC sentence");
    };
    assert!(datetime.is_none());
}

#[test]
fn a_bad_checksum_is_rejected() {
    let line = "$GPRMC,120000.000,A,4152.6300,N,08739.4200,W,0.0,0.0,050126,,,A*00";
    assert!(nmea::parse(line).is_none());
}

#[test]
fn gga_still_parses_alongside_rmc() {
    let line = sentence("GPGGA,120000.000,4152.6300,N,08739.4200,W,1,08,0.9,180.0,M,-33.0,M,,");
    let Some(Nmea::Gga {
        fix_quality,
        hdop,
        lat,
        lon,
    }) = nmea::parse(&line)
    else {
        panic!("expected a GGA sentence");
    };
    assert_eq!(fix_quality, 1);
    assert_eq!(hdop, Some(0.9));
    assert!(lat.is_some() && lon.is_some());
}

#[test]
fn any_talker_prefix_is_accepted() {
    // Multi-constellation receivers emit GN/GL/GA, not just GP.
    let line = sentence("GNRMC,120000.000,A,4152.6300,N,08739.4200,W,0.0,0.0,050126,,,A");
    assert!(matches!(nmea::parse(&line), Some(Nmea::Rmc { .. })));
}

// ---------------------------------------------------------------------------
// Device identity
//
// Every unit used to ship as `"eld-prototype-001"`. Ingest keys records and
// dedup on `device_id`, so two units in the field collided and no record could
// be attributed to a specific device.

#[test]
fn device_id_is_derived_from_the_chip_mac() {
    use eld_decoders_test::identity::format_device_id;

    assert_eq!(
        format_device_id([0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6]),
        "eld-a1b2c3d4e5f6"
    );
    // Low bytes keep their leading zero, or two different chips could collide.
    assert_eq!(
        format_device_id([0x00, 0x01, 0x02, 0x03, 0x04, 0x05]),
        "eld-000102030405"
    );
}

#[test]
fn distinct_macs_produce_distinct_ids() {
    use eld_decoders_test::identity::format_device_id;

    let a = format_device_id([0x24, 0x6f, 0x28, 0x00, 0x00, 0x01]);
    let b = format_device_id([0x24, 0x6f, 0x28, 0x00, 0x00, 0x02]);
    assert_ne!(a, b);
}

#[test]
fn device_id_stays_short_enough_for_the_ios_att_mtu() {
    use eld_decoders_test::identity::format_device_id;

    // BLE response lines cap at 220 bytes and iOS negotiates ~185. The id is
    // repeated in every EVENT envelope, so its length is not cosmetic.
    let id = format_device_id([0xff; 6]);
    assert_eq!(id.len(), 16);
    assert!(id.is_ascii());
}
