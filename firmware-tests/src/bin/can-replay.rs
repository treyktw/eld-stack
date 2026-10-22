//! `can-replay`: decode a candump log with the firmware's own J1939/OBD code.
//!
//! ```text
//! cargo run --bin can-replay -- fixtures/synthetic-drive.log
//! cargo run --bin can-replay -- --quiet fixtures/synthetic-drive.log
//! ```

use std::process::ExitCode;

use eld_decoders_test::replay::replay;

fn main() -> ExitCode {
    let mut quiet = false;
    let mut path = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-q" | "--quiet" => quiet = true,
            "-h" | "--help" => {
                eprintln!("usage: can-replay [--quiet] <candump.log>");
                return ExitCode::SUCCESS;
            }
            other => path = Some(other.to_owned()),
        }
    }
    let Some(path) = path else {
        eprintln!("usage: can-replay [--quiet] <candump.log>");
        return ExitCode::from(2);
    };
    let log = match std::fs::read_to_string(&path) {
        Ok(log) => log,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let s = replay(&log);

    if !quiet {
        // Repetitive per-frame signals are capped so the interesting events
        // (TP reassembly, DM1, odometer arbitration) stay readable.
        let mut shown_rpm = 0;
        let mut shown_speed = 0;
        for event in &s.events {
            if event.contains("(EEC1/rpm)") {
                shown_rpm += 1;
                if shown_rpm > 3 {
                    continue;
                }
            }
            if event.contains("(CCVS/speed)") {
                shown_speed += 1;
                if shown_speed > 3 {
                    continue;
                }
            }
            println!("{event}");
        }
        println!("    ...   (rpm/speed frames after the first 3 elided)\n");
    }

    println!("== summary ==");
    println!(
        "frames: {} total, {} J1939, {} OBD-II, {} unparsable lines",
        s.frames, s.j1939_frames, s.obd_frames, s.skipped_lines
    );
    println!("VIN:          {}", s.vin.as_deref().unwrap_or("-"));
    for dtc in &s.faults {
        println!("DTC:          SPN {} FMI {} (occurrences {})", dtc.spn, dtc.fmi, dtc.oc);
    }
    let show = |v: Option<f32>, unit: &str| match v {
        Some(v) => format!("{v:.2} {unit}"),
        None => "-".into(),
    };
    println!("engine rpm:   {}", show(s.rpm, "rpm"));
    println!("wheel speed:  {}", show(s.speed_kph, "km/h"));
    println!("fuel level:   {}", show(s.fuel_percent, "%"));
    println!("engine hours: {}", show(s.engine_hours, "h"));
    println!(
        "odometer:     {}",
        match s.odometer_km {
            Some(km) => format!("{km:.3} km ({:.3} mi)", km / 1.609_344),
            None => "-".into(),
        }
    );
    for (source, value) in &s.obd_readings {
        println!("OBD-II:       0x{source:03X} {value:?}");
    }
    if let Some(m) = s.mileage {
        println!(
            "trip mileage: {:.3} mi via {} (confidence {:.2}: {})",
            m.miles,
            m.source.label(),
            m.confidence,
            m.notes
        );
    }
    ExitCode::SUCCESS
}
