//! Which signing key the device trusts.
//!
//! A public key is not secret, so it is easy to treat storing it as
//! uninteresting. It is not: anyone who can *replace* the key can sign firmware
//! themselves and the device accepts it. These tests are about integrity, not
//! confidentiality.

use eld_decoders_test::ota::key::{
    resolve, resolve_with, updates_possible, EfuseKeySlot, KeyError, KeySource,
    BUILD_TIME_PUBLIC_KEY, PUBLIC_KEY_LEN,
};

fn real_key() -> [u8; PUBLIC_KEY_LEN] {
    let mut k = [0u8; PUBLIC_KEY_LEN];
    k[0] = 0xAB;
    k[31] = 0xCD;
    k
}

#[test]
fn a_write_protected_efuse_key_is_trusted() {
    let slot = EfuseKeySlot {
        key: real_key(),
        write_protected: true,
    };
    let trusted = resolve(Some(slot)).expect("should trust a protected key");
    assert_eq!(trusted.key, real_key());
    assert_eq!(trusted.source, KeySource::Efuse);
}

#[test]
fn a_writable_efuse_key_is_refused_not_silently_downgraded() {
    // The case this module exists for. A key in a block anything can rewrite is
    // an attacker-controlled key. Falling back to the build-time key would mean
    // a unit that looks provisioned behaves like one that is not, and nobody
    // would find out.
    let slot = EfuseKeySlot {
        key: real_key(),
        write_protected: false,
    };
    assert_eq!(resolve(Some(slot)), Err(KeyError::EfuseNotWriteProtected));
    assert!(!updates_possible(Some(slot)));
}

#[test]
fn an_empty_efuse_block_falls_back_to_the_build_time_key() {
    // Normal state for a unit built before provisioning exists.
    let slot = EfuseKeySlot {
        key: [0u8; PUBLIC_KEY_LEN],
        write_protected: false,
    };

    match resolve(Some(slot)) {
        // With no build-time key configured, this is the fail-closed refusal.
        Err(KeyError::NotConfigured) => {
            assert_eq!(BUILD_TIME_PUBLIC_KEY, [0u8; PUBLIC_KEY_LEN]);
        }
        Ok(trusted) => {
            assert_eq!(trusted.source, KeySource::BuildTime);
            assert_eq!(trusted.key, BUILD_TIME_PUBLIC_KEY);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn an_empty_block_is_not_treated_as_a_key_even_when_protected() {
    // A write-protected block of zeros is still not a key. Trusting it would
    // mean verifying signatures against all-zeros.
    let slot = EfuseKeySlot {
        key: [0u8; PUBLIC_KEY_LEN],
        write_protected: true,
    };
    assert!(
        !matches!(resolve(Some(slot)), Ok(t) if t.source == KeySource::Efuse),
        "zeros must never be accepted as an eFuse key"
    );
}

#[test]
fn no_key_anywhere_fails_closed() {
    // A device that cannot authenticate an update must install nothing, rather
    // than install everything. Exercised through resolve_with so the branch
    // stays covered now that a real key is compiled in — an untested refusal
    // path is how a refusal quietly becomes an accept.
    let unset = [0u8; PUBLIC_KEY_LEN];
    assert_eq!(resolve_with(None, &unset), Err(KeyError::NotConfigured));

    let empty_slot = EfuseKeySlot {
        key: unset,
        write_protected: true,
    };
    assert_eq!(
        resolve_with(Some(empty_slot), &unset),
        Err(KeyError::NotConfigured)
    );
}

#[test]
fn a_writable_efuse_key_is_refused_even_with_a_build_time_key_available() {
    // The refusal must not depend on there being no fallback. This is the
    // difference between "refused" and "quietly downgraded".
    let slot = EfuseKeySlot {
        key: real_key(),
        write_protected: false,
    };
    assert_eq!(
        resolve_with(Some(slot), &BUILD_TIME_PUBLIC_KEY),
        Err(KeyError::EfuseNotWriteProtected)
    );
}

#[test]
fn the_committed_key_is_a_valid_ed25519_point() {
    // Catches a mistyped or truncated paste from generate-signing-key.sh. A
    // malformed key compiles fine and then rejects every signature in the
    // field, which looks identical to an attack.
    assert!(
        ed25519_dalek::VerifyingKey::from_bytes(&BUILD_TIME_PUBLIC_KEY).is_ok(),
        "BUILD_TIME_PUBLIC_KEY is not a valid Ed25519 public key"
    );
}

#[test]
fn efuse_is_preferred_over_the_build_time_key() {
    // eFuse survives a firmware replacement; a compiled-in key does not.
    let slot = EfuseKeySlot {
        key: real_key(),
        write_protected: true,
    };
    let trusted = resolve(Some(slot)).unwrap();
    assert_eq!(trusted.source, KeySource::Efuse);
    assert_ne!(trusted.key, BUILD_TIME_PUBLIC_KEY);
}
