//! Firmware image authenticity.
//!
//! A SHA-256 proves the bytes arrived intact from whoever served them. It does
//! not prove *who* served them. Without a signature, anyone who controls the
//! update endpoint — or any CA in the device's trust store — can run arbitrary
//! code on every truck in the fleet, and the digest check will happily confirm
//! their image downloaded correctly.
//!
//! This module is the part of that fix which can be wrong in interesting ways:
//! deciding *what* is signed. The cryptography itself is delegated to a
//! [`Verifier`] so it can be a real implementation on the device and in tests,
//! and so nothing here is tempted to hand-roll a primitive.
//!
//! # What is signed, and why it is not the image
//!
//! The signature covers the **manifest**, not the image bytes, and the manifest
//! contains the image's digest. That indirection matters:
//!
//! - Signing the image alone leaves the version unsigned, so an attacker can
//!   serve a genuinely-signed *old* image and roll a truck back onto a build
//!   with a known vulnerability. The signature would verify perfectly.
//! - The device would also have to download the whole image before it could
//!   check whether it was authentic — metered cellular spent on bytes that may
//!   be hostile.
//!
//! Signing `version || digest || size || url` binds all of it together: the
//! image is authenticated transitively through its digest, and the version is
//! authenticated directly, so [`super::version::should_install`] is deciding on
//! a number the attacker cannot choose.
//!
//! # What this still does not defend against
//!
//! A signing key that leaks. Key custody, and Secure Boot to stop an attacker
//! with physical access simply flashing past all of this, are separate problems
//! — tracked as hardware decisions outside this repository.

use super::manifest::Manifest;

/// Domain separation prefix.
///
/// Included so a signature produced for some other artifact in this system — an ingest
/// payload, a config blob, a future message type — can never be replayed as a
/// firmware manifest signature. Without a prefix, any two formats that happen
/// to produce the same bytes are interchangeable to a verifier.
pub const SIGNING_CONTEXT: &str = "j1939-eld-fw-v1";

/// Ed25519 signature length.
pub const SIGNATURE_LEN: usize = 64;

/// Ed25519 public key length.
pub const PUBLIC_KEY_LEN: usize = 32;

/// Longest canonical message this builds. Bounded by the manifest's own limits.
pub const MAX_SIGNED_LEN: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// No signature field in the document.
    Missing,
    /// Signature was not 64 bytes of valid base64.
    Malformed,
    /// Canonical message did not fit — refuse rather than sign a truncation.
    TooLong,
    /// Cryptographic verification failed.
    Invalid,
}

/// Verifies an Ed25519 signature.
///
/// A trait so the device implementation and the test implementation are the
/// same code path, and so this module never contains a primitive of its own.
pub trait Verifier {
    fn verify(
        &self,
        message: &[u8],
        signature: &[u8; SIGNATURE_LEN],
        public_key: &[u8; PUBLIC_KEY_LEN],
    ) -> bool;
}

/// Build the exact bytes that are signed.
///
/// Newline-separated fields with a leading context string. Deliberately not
/// canonical JSON: canonicalising JSON is a well-known source of signature
/// bypasses — duplicate keys, unicode escapes, whitespace, key ordering — and
/// none of that risk buys anything here.
///
/// Every field is length-checked by the manifest parser before it reaches this
/// point, and none may contain a newline, so the separator is unambiguous.
pub fn canonical_message(
    manifest: &Manifest,
    out: &mut [u8; MAX_SIGNED_LEN],
) -> Result<usize, SignatureError> {
    let mut cursor = 0usize;

    let mut push = |bytes: &[u8], cursor: &mut usize| -> Result<(), SignatureError> {
        if *cursor + bytes.len() > MAX_SIGNED_LEN {
            return Err(SignatureError::TooLong);
        }
        out[*cursor..*cursor + bytes.len()].copy_from_slice(bytes);
        *cursor += bytes.len();
        Ok(())
    };

    push(SIGNING_CONTEXT.as_bytes(), &mut cursor)?;
    push(b"\n", &mut cursor)?;

    // Version first: this is the field that stops a rollback, so it is part of
    // the signed message rather than metadata alongside it.
    let mut digits = [0u8; 24];
    let version_len = write_version(manifest, &mut digits);
    push(&digits[..version_len], &mut cursor)?;
    push(b"\n", &mut cursor)?;

    push(manifest.sha256.as_bytes(), &mut cursor)?;
    push(b"\n", &mut cursor)?;

    let mut size_buf = [0u8; 10];
    let size_len = write_u32(manifest.size, &mut size_buf);
    push(&size_buf[..size_len], &mut cursor)?;
    push(b"\n", &mut cursor)?;

    push(manifest.url.as_bytes(), &mut cursor)?;
    push(b"\n", &mut cursor)?;

    Ok(cursor)
}

fn write_version(manifest: &Manifest, out: &mut [u8; 24]) -> usize {
    let mut n = 0;
    for (i, part) in [
        manifest.version.major,
        manifest.version.minor,
        manifest.version.patch,
    ]
    .iter()
    .enumerate()
    {
        if i > 0 {
            out[n] = b'.';
            n += 1;
        }
        let mut buf = [0u8; 5];
        let len = write_u32(*part as u32, &mut buf);
        out[n..n + len].copy_from_slice(&buf[..len]);
        n += len;
    }
    n
}

fn write_u32(mut value: u32, out: &mut [u8]) -> usize {
    if value == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut n = 0;
    while value > 0 {
        tmp[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    n
}

/// Decode the `"sig"` field: base64 of exactly 64 bytes.
pub fn decode_signature(document: &str) -> Result<[u8; SIGNATURE_LEN], SignatureError> {
    let value = extract_sig_field(document).ok_or(SignatureError::Missing)?;
    let mut out = [0u8; SIGNATURE_LEN];
    let written = base64_decode(value, &mut out).ok_or(SignatureError::Malformed)?;
    if written != SIGNATURE_LEN {
        return Err(SignatureError::Malformed);
    }
    Ok(out)
}

fn extract_sig_field(document: &str) -> Option<&str> {
    let bytes = document.as_bytes();
    let mut from = 0usize;
    while let Some(found) = document[from..].find("sig") {
        let start = from + found;
        let end = start + 3;
        let quoted =
            start > 0 && bytes[start - 1] == b'"' && end < bytes.len() && bytes[end] == b'"';
        if quoted {
            let mut cursor = end + 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b':' {
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                if cursor < bytes.len() && bytes[cursor] == b'"' {
                    let content = &document[cursor + 1..];
                    let close = content.find('"')?;
                    return Some(&content[..close]);
                }
            }
        }
        from = end;
    }
    None
}

/// Standard base64 with padding. Rejects any character outside the alphabet
/// rather than skipping it — a decoder that ignores junk lets an attacker pad a
/// signature with noise that different implementations read differently.
fn base64_decode(input: &str, out: &mut [u8]) -> Option<usize> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    if bytes.is_empty() || bytes.len() & 3 != 0 {
        return None;
    }

    let mut written = 0usize;
    let mut chunk = [0u8; 4];

    for block in bytes.chunks(4) {
        let mut pad = 0;
        for (i, &b) in block.iter().enumerate() {
            if b == b'=' {
                // Padding is only legal in the final two positions.
                if i < 2 {
                    return None;
                }
                pad += 1;
                chunk[i] = 0;
            } else {
                if pad > 0 {
                    return None; // data after padding
                }
                chunk[i] = value(b)?;
            }
        }

        let triple = ((chunk[0] as u32) << 18)
            | ((chunk[1] as u32) << 12)
            | ((chunk[2] as u32) << 6)
            | chunk[3] as u32;

        let produced = 3 - pad;
        for i in 0..produced {
            if written >= out.len() {
                return None;
            }
            out[written] = ((triple >> (16 - 8 * i)) & 0xff) as u8;
            written += 1;
        }
    }
    Some(written)
}

/// Verify a manifest against the device's public key.
///
/// Fails closed on every path. A caller that gets `Ok(())` knows the manifest —
/// and therefore the digest, and therefore the image — was produced by the
/// holder of the signing key.
pub fn verify_manifest<V: Verifier>(
    verifier: &V,
    document: &str,
    manifest: &Manifest,
    public_key: &[u8; PUBLIC_KEY_LEN],
) -> Result<(), SignatureError> {
    let signature = decode_signature(document)?;

    let mut buffer = [0u8; MAX_SIGNED_LEN];
    let len = canonical_message(manifest, &mut buffer)?;

    if verifier.verify(&buffer[..len], &signature, public_key) {
        Ok(())
    } else {
        Err(SignatureError::Invalid)
    }
}
