//! Which signing key the device trusts, and when it refuses to trust any.
//!
//! Pure — no eFuse, no FFI — so every refusal path is reachable from a host
//! test. The FFI that actually reads eFuse lives in [`super::verifier`] and is
//! gated on `target_os = "espidf"`.
//!
//! # Why a public key still needs protecting
//!
//! A public key is not secret, so it is tempting to treat storing it as
//! uninteresting. It is not: an attacker who can *replace* the public key can
//! sign firmware with a key of their own and the device will accept it happily.
//! Confidentiality does not matter here; integrity is everything.
//!
//! Two places a key can live, with different integrity stories:
//!
//! - **Compiled into the image.** Its integrity is the image's integrity, which
//!   means it is only as good as Secure Boot. Without Secure Boot, anyone who
//!   can flash the device replaces the key and the firmware together.
//! - **Burned into eFuse.** Integrity comes from the write-protect bit, and it
//!   survives a firmware replacement — which is what makes it worth the trouble.
//!
//! The subtlety this module exists for: **an eFuse key in a block that is not
//! write-protected is worth nothing.** It can be rewritten by anyone who can
//! run code on the chip, so trusting it is strictly worse than trusting the
//! compiled-in key, because it looks stronger while being weaker.

/// Ed25519 public key length.
pub const PUBLIC_KEY_LEN: usize = 32;

/// Build-time key, for units shipped before eFuse provisioning exists.
///
/// This is the public half of a **TEST** signing keypair generated for this
/// public repository (see `scripts/gen-ota-key.sh`). Its private half was
/// discarded and is not in the repo; replace this array with your own key
/// before building for real hardware. It is public by definition — there is
/// nothing to protect here by hiding it, and committing it is what lets a build
/// verify against the same key the devices use.
///
/// Its integrity, however, is only as strong as the image it sits in. Anyone
/// who can reflash a device without Secure Boot replaces this key and their
/// firmware together. That is exactly the gap eFuse closes, and why
/// [`resolve`] prefers a write-protected eFuse key over this one.
///
/// All zeros remains the "not configured" sentinel. Relying on the crypto to
/// reject a zero key would surface the refusal as a verification failure, which
/// looks identical to a tampered manifest — and "nobody provisioned a key" and
/// "someone attacked us" need different responses.
pub const BUILD_TIME_PUBLIC_KEY: [u8; PUBLIC_KEY_LEN] = [
    0x9e, 0xca, 0xb9, 0x55, 0x28, 0xa8, 0x4e, 0x93, 0x1f, 0x94, 0x67, 0x11, 0xe7, 0xfb, 0xd7, 0x00,
    0x24, 0x92, 0xeb, 0x1b, 0x1b, 0xfc, 0x7f, 0x68, 0x23, 0x64, 0xa4, 0xd5, 0x66, 0xfa, 0xa5, 0x06,
];

/// What eFuse reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EfuseKeySlot {
    pub key: [u8; PUBLIC_KEY_LEN],
    /// Whether the block can still be written.
    ///
    /// A readable-but-writable block is an attacker-controlled key.
    pub write_protected: bool,
}

/// Where the trusted key came from. Reported so a field failure can be
/// diagnosed without guessing which key the unit was using.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Efuse,
    BuildTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// No key anywhere. Updates must not proceed.
    NotConfigured,
    /// eFuse holds a key but the block is still writable, so it proves nothing.
    EfuseNotWriteProtected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedKey {
    pub key: [u8; PUBLIC_KEY_LEN],
    pub source: KeySource,
}

fn is_unset(key: &[u8; PUBLIC_KEY_LEN]) -> bool {
    key.iter().all(|&b| b == 0)
}

/// Decide which key to trust.
///
/// Order and refusals, all deliberate:
///
/// 1. A write-protected eFuse key wins. It survives a firmware replacement,
///    which the compiled-in key does not.
/// 2. A **writable** eFuse key is an error, not a fallback. Silently dropping
///    back to the build-time key would mean a unit that looks provisioned
///    behaves like one that is not, and the operator would never find out.
/// 3. An empty eFuse block falls back to the build-time key — that is the
///    normal state for a unit built before provisioning exists.
/// 4. No key anywhere refuses. Fail closed: a device that cannot authenticate
///    an update must install nothing, not install everything.
pub fn resolve(efuse: Option<EfuseKeySlot>) -> Result<TrustedKey, KeyError> {
    resolve_with(efuse, &BUILD_TIME_PUBLIC_KEY)
}

/// [`resolve`], with the build-time key supplied explicitly.
///
/// Exists so the "no key anywhere" branch stays reachable from a test even
/// though a key is now compiled in. That branch is the fail-closed guarantee;
/// leaving it untested because the happy path is configured is how a refusal
/// path quietly rots into an accept path.
pub fn resolve_with(
    efuse: Option<EfuseKeySlot>,
    build_time_key: &[u8; PUBLIC_KEY_LEN],
) -> Result<TrustedKey, KeyError> {
    if let Some(slot) = efuse {
        if !is_unset(&slot.key) {
            if !slot.write_protected {
                return Err(KeyError::EfuseNotWriteProtected);
            }
            return Ok(TrustedKey {
                key: slot.key,
                source: KeySource::Efuse,
            });
        }
    }

    if is_unset(build_time_key) {
        return Err(KeyError::NotConfigured);
    }

    Ok(TrustedKey {
        key: *build_time_key,
        source: KeySource::BuildTime,
    })
}

/// Whether this build can authenticate updates at all.
///
/// Intended for a startup log line. A unit that will silently never update is
/// worth surfacing at boot rather than discovering months later when a security
/// fix does not land.
pub fn updates_possible(efuse: Option<EfuseKeySlot>) -> bool {
    resolve(efuse).is_ok()
}
