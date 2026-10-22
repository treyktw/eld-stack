//! Unit tests for the pure J1939 decoders: VIN, DM1, and TP (BAM) reassembly.

use eld_decoders_test::j1939::decoder::{self, Dtc};
use eld_decoders_test::j1939::id::J1939Id;
use eld_decoders_test::j1939::pgn;
use eld_decoders_test::j1939::transport::TransportReassembler;

// ---- helpers --------------------------------------------------------------

/// Build a J1939Id for a TP frame (PDU1; destination carried separately).
fn tp_id(pgn: u32, source: u8) -> J1939Id {
    J1939Id {
        priority: 6,
        pgn,
        source,
        destination: Some(0xFF), // BAM is global
    }
}

/// TP.CM BAM announcement: size, packet count, and the data PGN (LE).
fn bam_cm(size: u16, packets: u8, data_pgn: u32) -> [u8; 8] {
    [
        0x20, // control byte = BAM
        (size & 0xFF) as u8,
        (size >> 8) as u8,
        packets,
        0xFF,
        (data_pgn & 0xFF) as u8,
        ((data_pgn >> 8) & 0xFF) as u8,
        ((data_pgn >> 16) & 0xFF) as u8,
    ]
}

/// One TP.DT data packet: 1 sequence byte + up to 7 payload bytes (0xFF-padded).
fn dt_packet(seq: u8, chunk: &[u8]) -> [u8; 8] {
    let mut out = [0xFFu8; 8];
    out[0] = seq;
    out[1..1 + chunk.len()].copy_from_slice(chunk);
    out
}

/// Encode a 4-byte DM1 DTC record using the v4 SPN conversion.
fn dtc_bytes(spn: u32, fmi: u8, oc: u8) -> [u8; 4] {
    [
        (spn & 0xFF) as u8,
        ((spn >> 8) & 0xFF) as u8,
        (((spn >> 16) & 0x07) as u8) << 5 | (fmi & 0x1F),
        oc & 0x7F,
    ]
}

/// Feed a multi-packet BAM message (CM + all DT packets) through a fresh
/// reassembler and return the completed message's reassembled bytes.
fn reassemble(data_pgn: u32, source: u8, payload: &[u8]) -> Vec<u8> {
    let mut tp = TransportReassembler::new();
    let packets = payload.len().div_ceil(7) as u8;
    let cm = bam_cm(payload.len() as u16, packets, data_pgn);
    assert!(tp.ingest(&tp_id(pgn::TP_CM, source), &cm).is_none());

    let mut last = None;
    for (i, chunk) in payload.chunks(7).enumerate() {
        last = tp.ingest(&tp_id(pgn::TP_DT, source), &dt_packet(i as u8 + 1, chunk));
    }
    let msg = last.expect("final TP.DT packet should complete the message");
    assert_eq!(msg.pgn, data_pgn);
    assert_eq!(msg.source, source);
    msg.data.to_vec()
}

// ---- VIN ------------------------------------------------------------------

#[test]
fn vin_decodes_star_terminated() {
    let vin = decoder::decode_vin(b"TSTEST015T0000001*tail-garbage").unwrap();
    assert_eq!(vin.as_str(), "TSTEST015T0000001");
}

#[test]
fn vin_stops_at_nul_and_0xff_padding() {
    assert_eq!(
        decoder::decode_vin(b"TSTEST026T0000002\x00junk")
            .unwrap()
            .as_str(),
        "TSTEST026T0000002"
    );
    assert_eq!(
        decoder::decode_vin(b"TSTEST037T0000003\xff\xff")
            .unwrap()
            .as_str(),
        "TSTEST037T0000003"
    );
}

#[test]
fn vin_trims_surrounding_whitespace() {
    assert_eq!(
        decoder::decode_vin(b"  TSTEST048T0000004  *")
            .unwrap()
            .as_str(),
        "TSTEST048T0000004"
    );
}

#[test]
fn vin_empty_or_blank_is_none() {
    assert!(decoder::decode_vin(b"").is_none());
    assert!(decoder::decode_vin(b"*").is_none());
    assert!(decoder::decode_vin(b"   ").is_none());
    assert!(decoder::decode_vin(&[0x00, 0x00]).is_none());
}

#[test]
fn vin_caps_at_max_len() {
    // 200 'A's, no terminator: decoder must stop at MAX_VIN, not overflow.
    let long = vec![b'A'; 200];
    let vin = decoder::decode_vin(&long).unwrap();
    assert_eq!(vin.len(), decoder::MAX_VIN);
}

// ---- DM1 ------------------------------------------------------------------

#[test]
fn dm1_decodes_single_dtc() {
    let mut payload = vec![0x00, 0x00]; // two lamp-status bytes
    payload.extend_from_slice(&dtc_bytes(1234, 5, 3));
    let faults = decoder::decode_dm1(&payload);
    assert_eq!(faults.len(), 1);
    assert_eq!(
        faults[0],
        Dtc {
            spn: 1234,
            fmi: 5,
            oc: 3
        }
    );
}

#[test]
fn dm1_decodes_high_spn_using_top_bits() {
    // SPN 0x12345 exercises the high-3-bits field of byte 2.
    let mut payload = vec![0xFF, 0xFF];
    payload.extend_from_slice(&dtc_bytes(0x12345, 2, 1));
    let faults = decoder::decode_dm1(&payload);
    assert_eq!(faults.len(), 1);
    assert_eq!(
        faults[0],
        Dtc {
            spn: 0x12345,
            fmi: 2,
            oc: 1
        }
    );
}

#[test]
fn dm1_decodes_multiple_dtcs() {
    let mut payload = vec![0x00, 0x00];
    payload.extend_from_slice(&dtc_bytes(100, 1, 1));
    payload.extend_from_slice(&dtc_bytes(200, 4, 9));
    payload.extend_from_slice(&dtc_bytes(630, 12, 2));
    let faults = decoder::decode_dm1(&payload);
    assert_eq!(faults.len(), 3);
    assert_eq!(faults[0].spn, 100);
    assert_eq!(faults[1].spn, 200);
    assert_eq!(
        faults[2],
        Dtc {
            spn: 630,
            fmi: 12,
            oc: 2
        }
    );
}

#[test]
fn dm1_skips_padding_and_empty_records() {
    let mut payload = vec![0x00, 0x00];
    payload.extend_from_slice(&dtc_bytes(515, 3, 1));
    payload.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // "no more faults" pad
    payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // empty record
    let faults = decoder::decode_dm1(&payload);
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].spn, 515);
}

#[test]
fn dm1_too_short_is_empty() {
    assert!(decoder::decode_dm1(&[]).is_empty());
    assert!(decoder::decode_dm1(&[0x00]).is_empty());
    // Lamp bytes only, no DTC.
    assert!(decoder::decode_dm1(&[0x00, 0x00]).is_empty());
}

#[test]
fn dm1_caps_at_max_dtcs() {
    let mut payload = vec![0x00, 0x00];
    for i in 0..(decoder::MAX_DTCS as u32 + 5) {
        payload.extend_from_slice(&dtc_bytes(100 + i, 1, 1));
    }
    let faults = decoder::decode_dm1(&payload);
    assert_eq!(faults.len(), decoder::MAX_DTCS);
}

// ---- TP (BAM) reassembly --------------------------------------------------

#[test]
fn tp_reassembles_vin_across_packets() {
    let payload = b"TSTEST015T0000001*"; // 18 bytes -> 3 packets
    let data = reassemble(pgn::VEHICLE_ID, 0x00, payload);
    assert_eq!(data, payload);
    assert_eq!(
        decoder::decode_vin(&data).unwrap().as_str(),
        "TSTEST015T0000001"
    );
}

#[test]
fn tp_reassembles_multipacket_dm1() {
    // Two lamp bytes + 3 DTCs = 14 bytes -> 2 packets.
    let mut payload = vec![0x00, 0x00];
    payload.extend_from_slice(&dtc_bytes(100, 1, 1));
    payload.extend_from_slice(&dtc_bytes(200, 4, 2));
    payload.extend_from_slice(&dtc_bytes(300, 5, 3));
    let data = reassemble(pgn::DM1, 0x17, &payload);
    let faults = decoder::decode_dm1(&data);
    assert_eq!(faults.len(), 3);
    assert_eq!(
        faults[2],
        Dtc {
            spn: 300,
            fmi: 5,
            oc: 3
        }
    );
}

#[test]
fn tp_interleaves_two_sources() {
    let mut tp = TransportReassembler::new();
    let vin_a = b"AAAAAAAAAAAAAAAAA*"; // 18 bytes, 3 packets, source 0x01
    let vin_b = b"BBBBBBBBBBBBBBBBB*"; // 18 bytes, 3 packets, source 0x02

    tp.ingest(
        &tp_id(pgn::TP_CM, 0x01),
        &bam_cm(vin_a.len() as u16, 3, pgn::VEHICLE_ID),
    );
    tp.ingest(
        &tp_id(pgn::TP_CM, 0x02),
        &bam_cm(vin_b.len() as u16, 3, pgn::VEHICLE_ID),
    );

    let mut done_a = None;
    let mut done_b = None;
    for i in 0..3 {
        let ca = &vin_a[i * 7..((i + 1) * 7).min(vin_a.len())];
        let cb = &vin_b[i * 7..((i + 1) * 7).min(vin_b.len())];
        let ra = tp.ingest(&tp_id(pgn::TP_DT, 0x01), &dt_packet(i as u8 + 1, ca));
        let rb = tp.ingest(&tp_id(pgn::TP_DT, 0x02), &dt_packet(i as u8 + 1, cb));
        if ra.is_some() {
            done_a = ra;
        }
        if rb.is_some() {
            done_b = rb;
        }
    }
    assert_eq!(done_a.unwrap().data.as_slice(), vin_a);
    assert_eq!(done_b.unwrap().data.as_slice(), vin_b);
}

#[test]
fn tp_ignores_dt_without_announcement() {
    let mut tp = TransportReassembler::new();
    // A stray data packet with no preceding BAM must not produce a message.
    assert!(tp
        .ingest(&tp_id(pgn::TP_DT, 0x05), &dt_packet(1, b"orphan"))
        .is_none());
}

#[test]
fn tp_rejects_oversize_announcement() {
    let mut tp = TransportReassembler::new();
    // size beyond the reassembly cap must be dropped (no panic, no session).
    tp.ingest(
        &tp_id(pgn::TP_CM, 0x06),
        &bam_cm(60_000, 255, pgn::VEHICLE_ID),
    );
    assert!(tp
        .ingest(&tp_id(pgn::TP_DT, 0x06), &dt_packet(1, b"data"))
        .is_none());
}

// ---- decode() sanity (scaling) -------------------------------------------

#[test]
fn decode_engine_rpm_scaling() {
    // EEC1 SPN 190 lives in bytes 4-5 (LE), 0.125 rpm/bit. raw 0x2000 = 8192.
    let id = J1939Id::parse(0x0CF00400).unwrap();
    let data = [0, 0, 0, 0x00, 0x20, 0, 0, 0];
    match decoder::decode(&id, &data) {
        decoder::J1939Signal::EngineRpm { rpm, .. } => assert!((rpm - 1024.0).abs() < 0.01),
        other => panic!("expected EngineRpm, got {other:?}"),
    }
}
