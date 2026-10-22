//! Manifest parsing — the least trusted input the device accepts.
//!
//! These tests are written adversarially on purpose. A manifest decides which
//! code runs on a truck, so the interesting cases are not the well-formed ones.

use eld_decoders_test::ota::manifest::{parse, ManifestError, MAX_IMAGE_BYTES};
use eld_decoders_test::ota::version::Version;

const DIGEST: &str = "1cce4833e976a8075afecf3105866b4fc5e15a93ceb19af45d23301668a4d651";

fn good() -> String {
    format!(
        r#"{{"version":"0.1.8","url":"https://updates.example.com/firmware/0.1.8.bin","sha256":"{DIGEST}","size":1048576,"required":false}}"#
    )
}

#[test]
fn parses_a_well_formed_manifest() {
    let m = parse(&good()).expect("should parse");
    assert_eq!(m.version, Version::new(0, 1, 8));
    assert_eq!(
        m.url.as_str(),
        "https://updates.example.com/firmware/0.1.8.bin"
    );
    assert_eq!(m.sha256.as_str(), DIGEST);
    assert_eq!(m.size, 1_048_576);
    assert!(!m.required);
}

#[test]
fn tolerates_whitespace_and_field_reordering() {
    let doc = format!(
        "{{\n  \"required\" : true ,\n  \"size\" : 2048 ,\n  \"sha256\" : \"{DIGEST}\" ,\n  \"url\" : \"https://updates.example.com/f.bin\" ,\n  \"version\" : \"1.0.0\"\n}}"
    );
    let m = parse(&doc).expect("should parse");
    assert_eq!(m.version, Version::new(1, 0, 0));
    assert!(m.required);
    assert_eq!(m.size, 2048);
}

#[test]
fn missing_required_flag_defaults_to_not_required() {
    // A field the server forgot must resolve to the less disruptive behaviour.
    let doc = format!(
        r#"{{"version":"0.1.8","url":"https://a.example/f.bin","sha256":"{DIGEST}","size":100}}"#
    );
    assert!(!parse(&doc).expect("should parse").required);
}

#[test]
fn rejects_plain_http() {
    // An image fetched over HTTP can be swapped in flight, and the digest is no
    // help when the manifest travelled the same way.
    let doc = good().replace("https://", "http://");
    assert_eq!(parse(&doc), Err(ManifestError::BadUrl));
}

#[test]
fn rejects_userinfo_in_the_authority() {
    // The classic way to make a hostile host look like a familiar one.
    let doc = good().replace(
        "https://updates.example.com/firmware/0.1.8.bin",
        "https://updates.example.com@evil.example/f.bin",
    );
    assert_eq!(parse(&doc), Err(ManifestError::BadUrl));
}

#[test]
fn accepts_an_at_sign_inside_the_path() {
    // Only the authority is dangerous; a path may legitimately contain '@'.
    let doc = good().replace(
        "https://updates.example.com/firmware/0.1.8.bin",
        "https://updates.example.com/firmware/build@0.1.8.bin",
    );
    assert!(parse(&doc).is_ok());
}

#[test]
fn rejects_bad_digests() {
    for bad in [
        "",
        "abc",
        // 63 chars
        &DIGEST[..63],
        // uppercase — refused rather than normalised, so comparison stays a
        // plain byte equality
        &DIGEST.to_uppercase(),
        // right length, non-hex
        &"z".repeat(64),
    ] {
        let doc = good().replace(DIGEST, bad);
        assert_eq!(
            parse(&doc),
            Err(ManifestError::BadDigest),
            "expected {bad:?} to be rejected"
        );
    }
}

#[test]
fn rejects_sizes_that_cannot_fit_the_slot() {
    let too_big = good().replace("1048576", &(MAX_IMAGE_BYTES + 1).to_string());
    assert_eq!(parse(&too_big), Err(ManifestError::BadSize));

    let zero = good().replace("1048576", "0");
    assert_eq!(parse(&zero), Err(ManifestError::BadSize));
}

#[test]
fn rejects_missing_fields() {
    let doc = format!(r#"{{"url":"https://a.example/f.bin","sha256":"{DIGEST}","size":10}}"#);
    assert_eq!(parse(&doc), Err(ManifestError::MissingField("version")));

    let doc = format!(r#"{{"version":"0.1.8","sha256":"{DIGEST}","size":10}}"#);
    assert_eq!(parse(&doc), Err(ManifestError::MissingField("url")));

    let doc = r#"{"version":"0.1.8","url":"https://a.example/f.bin","size":10}"#;
    assert_eq!(parse(doc), Err(ManifestError::MissingField("sha256")));
}

#[test]
fn does_not_mistake_a_value_for_a_key() {
    // A URL containing the text `version` must not be read as the version
    // field. This is why the parser matches on a quoted key plus a colon.
    let doc = format!(
        r#"{{"url":"https://updates.example.com/version/0.1.8.bin","version":"2.3.4","sha256":"{DIGEST}","size":10}}"#
    );
    let m = parse(&doc).expect("should parse");
    assert_eq!(m.version, Version::new(2, 3, 4));
}

#[test]
fn rejects_escapes_rather_than_half_understanding_them() {
    let doc = good().replace("0.1.8.bin", "0.1.8\\u002ebin");
    assert_eq!(parse(&doc), Err(ManifestError::Malformed));
}

#[test]
fn rejects_an_over_long_url() {
    let long = format!("https://a.example/{}", "x".repeat(300));
    let doc = good().replace("https://updates.example.com/firmware/0.1.8.bin", &long);
    assert_eq!(parse(&doc), Err(ManifestError::BadUrl));
}

#[test]
fn rejects_control_characters_in_the_url() {
    let doc = good().replace("0.1.8.bin", "0.1.8\u{7f}.bin");
    assert_eq!(parse(&doc), Err(ManifestError::BadUrl));
}

#[test]
fn rejects_empty_and_garbage_documents() {
    assert!(parse("").is_err());
    assert!(parse("{}").is_err());
    assert!(parse("not json at all").is_err());
}
