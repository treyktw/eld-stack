//! Update manifest parsing.
//!
//! The manifest is the least trusted input the device accepts. Everything else
//! it reads is sensor data that can be wrong; this decides which code runs on a
//! truck. So the parser is deliberately narrow and refuses anything it does not
//! fully understand, rather than doing its best with a partial document.
//!
//! Expected shape:
//!
//! ```json
//! { "version": "0.1.8",
//!   "url": "https://cdn.example.com/firmware/0.1.8.bin",
//!   "sha256": "<64 hex>",
//!   "size": 1234567,
//!   "required": false }
//! ```
//!
//! Written without serde: the firmware has no JSON dependency, and this grammar
//! is small enough that hand-parsing is less risk than pulling one in.
//!
//! **What this does not do:** validating a SHA-256 proves the bytes arrived
//! intact from whoever served them. It does not prove who served them. Until
//! images are signed and verified against a key in eFuse, anyone who can control
//! the update endpoint — or a TLS trust store — can run arbitrary code on every
//! truck.

use super::version::Version;
use heapless::String as HString;

/// Longest accepted download URL.
pub const MAX_URL_LEN: usize = 256;

/// Largest image this device will consider, in bytes.
///
/// An OTA slot is 3 MB (see `partitions.csv`). Refusing anything larger here
/// means a bad `size` is caught before a single byte is downloaded over metered
/// cellular, rather than after the partition write fails.
pub const MAX_IMAGE_BYTES: u32 = 3 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub url: HString<MAX_URL_LEN>,
    /// Lowercase hex, exactly 64 characters.
    pub sha256: HString<64>,
    pub size: u32,
    /// Carrier-mandated update. Still never installed while the truck moves.
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    MissingField(&'static str),
    BadVersion,
    BadUrl,
    BadDigest,
    BadSize,
    Malformed,
}

/// Parse and validate a manifest document.
pub fn parse(document: &str) -> Result<Manifest, ManifestError> {
    let version_text = string_field(document, "version")?;
    let version = Version::parse(version_text).ok_or(ManifestError::BadVersion)?;

    let url_text = string_field(document, "url")?;
    validate_url(url_text)?;
    let mut url = HString::new();
    url.push_str(url_text).map_err(|_| ManifestError::BadUrl)?;

    let digest_text = string_field(document, "sha256")?;
    validate_digest(digest_text)?;
    let mut sha256 = HString::new();
    sha256
        .push_str(digest_text)
        .map_err(|_| ManifestError::BadDigest)?;

    let size = number_field(document, "size")?;
    if size == 0 || size > MAX_IMAGE_BYTES {
        return Err(ManifestError::BadSize);
    }

    // Absent means not required. A missing flag must never be read as "install
    // this now" — the safe default for a field the server forgot to send is the
    // less disruptive one.
    let required = bool_field(document, "required").unwrap_or(false);

    Ok(Manifest {
        version,
        url,
        sha256,
        size,
        required,
    })
}

/// HTTPS only, no credentials, no control characters, length-bounded.
///
/// Plain HTTP is refused outright rather than warned about: an image fetched
/// over HTTP can be replaced in flight by anyone on the path, and the digest in
/// the manifest is no help when the manifest travelled the same way.
fn validate_url(url: &str) -> Result<(), ManifestError> {
    if url.len() > MAX_URL_LEN || url.len() < "https://a".len() {
        return Err(ManifestError::BadUrl);
    }
    if !url.starts_with("https://") {
        return Err(ManifestError::BadUrl);
    }
    // `@` before the first `/` of the path is a userinfo section, which is the
    // classic way to make a hostile host look like a familiar one.
    let authority = &url["https://".len()..];
    let authority_end = authority.find('/').unwrap_or(authority.len());
    if authority[..authority_end].contains('@') {
        return Err(ManifestError::BadUrl);
    }
    if authority_end == 0 {
        return Err(ManifestError::BadUrl);
    }
    if url
        .bytes()
        .any(|b| b.is_ascii_control() || b == b' ' || !b.is_ascii())
    {
        return Err(ManifestError::BadUrl);
    }
    Ok(())
}

/// Exactly 64 lowercase hex characters.
///
/// Case is fixed rather than normalised so the comparison against a locally
/// computed digest is a plain byte equality with no room for a case-folding bug.
fn validate_digest(digest: &str) -> Result<(), ManifestError> {
    if digest.len() != 64 {
        return Err(ManifestError::BadDigest);
    }
    if !digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ManifestError::BadDigest);
    }
    Ok(())
}

/// Find `"name"` used as a key and return the byte offset just past its colon.
///
/// Matching on the quoted key plus a colon avoids hitting the same text inside a
/// *value* — a URL containing `"version"` should not be mistaken for the version
/// field.
fn value_start(document: &str, name: &str) -> Option<usize> {
    let bytes = document.as_bytes();
    let mut from = 0usize;

    while let Some(found) = document[from..].find(name) {
        let key_start = from + found;
        let key_end = key_start + name.len();

        let quoted = key_start > 0
            && bytes[key_start - 1] == b'"'
            && key_end < bytes.len()
            && bytes[key_end] == b'"';

        if quoted {
            let mut cursor = key_end + 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b':' {
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                return Some(cursor);
            }
        }

        from = key_end;
    }
    None
}

fn string_field<'a>(document: &'a str, name: &'static str) -> Result<&'a str, ManifestError> {
    let start = value_start(document, name).ok_or(ManifestError::MissingField(name))?;
    let bytes = document.as_bytes();
    if start >= bytes.len() || bytes[start] != b'"' {
        return Err(ManifestError::Malformed);
    }
    let content_start = start + 1;
    let rest = &document[content_start..];

    // No escape handling: none of these fields may contain a backslash, so a
    // document with one is rejected rather than half-understood.
    let end = rest.find('"').ok_or(ManifestError::Malformed)?;
    let value = &rest[..end];
    if value.contains('\\') {
        return Err(ManifestError::Malformed);
    }
    Ok(value)
}

fn number_field(document: &str, name: &'static str) -> Result<u32, ManifestError> {
    let start = value_start(document, name).ok_or(ManifestError::MissingField(name))?;
    let rest = &document[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 || end > 10 {
        return Err(ManifestError::BadSize);
    }
    rest[..end]
        .parse::<u32>()
        .map_err(|_| ManifestError::BadSize)
}

fn bool_field(document: &str, name: &str) -> Option<bool> {
    let start = value_start(document, name)?;
    let rest = &document[start..];
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}
