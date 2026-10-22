//! Firmware manifest signing, against real Ed25519.
//!
//! A stub verifier would only prove the plumbing. These tests sign with a real
//! key so the canonical-message format is exercised end to end — if the bytes
//! the device signs and the bytes the build server signs ever diverge, that is
//! a fleet-wide outage, and it is the kind of bug a stub cannot catch.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier as DalekVerifier, VerifyingKey};
use eld_decoders_test::ota::manifest::{parse, Manifest};
use eld_decoders_test::ota::signature::{
    canonical_message, decode_signature, verify_manifest, SignatureError, Verifier, MAX_SIGNED_LEN,
    PUBLIC_KEY_LEN, SIGNATURE_LEN,
};

const DIGEST: &str = "1cce4833e976a8075afecf3105866b4fc5e15a93ceb19af45d23301668a4d651";

/// The device-side verifier, backed by real Ed25519.
struct Ed25519;

impl Verifier for Ed25519 {
    fn verify(
        &self,
        message: &[u8],
        signature: &[u8; SIGNATURE_LEN],
        public_key: &[u8; PUBLIC_KEY_LEN],
    ) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(public_key) else {
            return false;
        };
        key.verify(message, &Signature::from_bytes(signature))
            .is_ok()
    }
}

/// Deterministic key, so a failure is reproducible.
fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn manifest_json(version: &str, digest: &str, size: u32, url: &str) -> String {
    format!(
        r#"{{"version":"{version}","url":"{url}","sha256":"{digest}","size":{size},"required":false}}"#
    )
}

/// Sign a manifest exactly as a build server would: parse it, build the
/// canonical message, sign that, and append the signature.
fn sign(document: &str) -> (String, Manifest, [u8; PUBLIC_KEY_LEN]) {
    let manifest = parse(document).expect("fixture should parse");
    let mut buffer = [0u8; MAX_SIGNED_LEN];
    let len = canonical_message(&manifest, &mut buffer).expect("should fit");

    let key = signing_key();
    let sig: Signature = key.sign(&buffer[..len]);
    let signed = format!(
        "{},\"sig\":\"{}\"}}",
        document.trim_end_matches('}'),
        base64(&sig.to_bytes())
    );
    (signed, manifest, key.verifying_key().to_bytes())
}

#[test]
fn accepts_a_genuinely_signed_manifest() {
    let doc = manifest_json("0.1.8", DIGEST, 1024, "https://updates.example.com/f.bin");
    let (signed, manifest, public_key) = sign(&doc);
    assert_eq!(
        verify_manifest(&Ed25519, &signed, &manifest, &public_key),
        Ok(())
    );
}

#[test]
fn rejects_a_manifest_signed_by_the_wrong_key() {
    let doc = manifest_json("0.1.8", DIGEST, 1024, "https://updates.example.com/f.bin");
    let (signed, manifest, _) = sign(&doc);
    let attacker = SigningKey::from_bytes(&[9u8; 32])
        .verifying_key()
        .to_bytes();
    assert_eq!(
        verify_manifest(&Ed25519, &signed, &manifest, &attacker),
        Err(SignatureError::Invalid)
    );
}

#[test]
fn rejects_a_swapped_image_digest() {
    // The whole point of signing the manifest: the image is authenticated
    // through its digest, so substituting the image breaks the signature.
    let doc = manifest_json("0.1.8", DIGEST, 1024, "https://updates.example.com/f.bin");
    let (signed, _, public_key) = sign(&doc);

    let hostile = manifest_json(
        "0.1.8",
        "0000000000000000000000000000000000000000000000000000000000000000",
        1024,
        "https://updates.example.com/f.bin",
    );
    let hostile_manifest = parse(&hostile).unwrap();

    assert_eq!(
        verify_manifest(&Ed25519, &signed, &hostile_manifest, &public_key),
        Err(SignatureError::Invalid)
    );
}

#[test]
fn rejects_a_rollback_to_a_genuinely_signed_old_version() {
    // The attack that signing the image alone would not stop: serve a real,
    // correctly-signed older build to put a truck back on a vulnerable version.
    // Because the version is inside the signed message, the old signature does
    // not authenticate a manifest claiming a new version, and vice versa.
    let old = manifest_json("0.1.0", DIGEST, 1024, "https://updates.example.com/f.bin");
    let (old_signed, _, public_key) = sign(&old);

    let claims_new = parse(&manifest_json(
        "9.9.9",
        DIGEST,
        1024,
        "https://updates.example.com/f.bin",
    ))
    .unwrap();

    assert_eq!(
        verify_manifest(&Ed25519, &old_signed, &claims_new, &public_key),
        Err(SignatureError::Invalid),
        "an old signature must not authenticate a different version"
    );
}

#[test]
fn rejects_a_redirected_download_url() {
    // Repointing the URL at an attacker's host must invalidate the signature,
    // even though the digest is unchanged.
    let doc = manifest_json("0.1.8", DIGEST, 1024, "https://updates.example.com/f.bin");
    let (signed, _, public_key) = sign(&doc);

    let redirected = parse(&manifest_json(
        "0.1.8",
        DIGEST,
        1024,
        "https://evil.example/f.bin",
    ))
    .unwrap();

    assert_eq!(
        verify_manifest(&Ed25519, &signed, &redirected, &public_key),
        Err(SignatureError::Invalid)
    );
}

#[test]
fn rejects_a_changed_size() {
    let doc = manifest_json("0.1.8", DIGEST, 1024, "https://updates.example.com/f.bin");
    let (signed, _, public_key) = sign(&doc);
    let resized = parse(&manifest_json(
        "0.1.8",
        DIGEST,
        2048,
        "https://updates.example.com/f.bin",
    ))
    .unwrap();
    assert_eq!(
        verify_manifest(&Ed25519, &signed, &resized, &public_key),
        Err(SignatureError::Invalid)
    );
}

#[test]
fn rejects_an_unsigned_manifest() {
    let doc = manifest_json("0.1.8", DIGEST, 1024, "https://updates.example.com/f.bin");
    let manifest = parse(&doc).unwrap();
    let public_key = signing_key().verifying_key().to_bytes();
    assert_eq!(
        verify_manifest(&Ed25519, &doc, &manifest, &public_key),
        Err(SignatureError::Missing)
    );
}

#[test]
fn rejects_malformed_signatures() {
    for bad in [
        r#"{"sig":""}"#,
        r#"{"sig":"not-base64!!"}"#,
        // valid base64, wrong length
        r#"{"sig":"YWJjZA=="}"#,
        // data after padding
        r#"{"sig":"QUJDR=Q="}"#,
    ] {
        assert!(
            matches!(
                decode_signature(bad),
                Err(SignatureError::Malformed) | Err(SignatureError::Missing)
            ),
            "expected {bad} to be rejected"
        );
    }
}

#[test]
fn the_canonical_message_binds_every_field_that_matters() {
    // If any of these stopped being covered, one of the attacks above would
    // start passing. Asserting on the bytes makes that a test failure rather
    // than a silent weakening.
    let doc = manifest_json("1.2.3", DIGEST, 4096, "https://updates.example.com/f.bin");
    let manifest = parse(&doc).unwrap();
    let mut buf = [0u8; MAX_SIGNED_LEN];
    let len = canonical_message(&manifest, &mut buf).unwrap();
    let msg = std::str::from_utf8(&buf[..len]).unwrap();

    assert_eq!(
        msg,
        format!("j1939-eld-fw-v1\n1.2.3\n{DIGEST}\n4096\nhttps://updates.example.com/f.bin\n")
    );
}

#[test]
fn the_context_prefix_is_present_so_signatures_cannot_be_replayed() {
    // A signature made for another artifact must not verify here.
    let doc = manifest_json("0.1.8", DIGEST, 1024, "https://updates.example.com/f.bin");
    let manifest = parse(&doc).unwrap();
    let mut buf = [0u8; MAX_SIGNED_LEN];
    let len = canonical_message(&manifest, &mut buf).unwrap();
    assert!(buf[..len].starts_with(b"j1939-eld-fw-v1\n"));
}
