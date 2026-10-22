//! Device-side signature verification: the real Ed25519, and the eFuse read.
//!
//! Everything here is gated on `target_os = "espidf"` because it calls into
//! ESP-IDF. The decision logic it depends on — which key to trust — lives in
//! [`super::key`] precisely so it is testable without a device.
//!
//! # Key block choice
//!
//! `EFUSE_PUBLIC_KEY_BLOCK` names the block the public key is burned into.
//! On the original ESP32 that is BLK3, the general-purpose 256-bit user block;
//! on the S3 it is one of the `BLOCK_KEY*` slots. It must be a block Secure
//! Boot and Flash Encryption do not claim for themselves, or provisioning will
//! either fail or quietly destroy a key the bootloader needs.
//!
//! # Provisioning
//!
//! At manufacture, burn the 32-byte public key into the block and then burn its
//! write-protect bit. Both steps are required: an unprotected block is
//! rewritable by anything that can run code on the chip, which is a key an
//! attacker controls. `key::resolve` refuses that case rather than falling back,
//! so a half-provisioned unit fails loudly instead of appearing provisioned.
//!
//! Burning write-protection is irreversible. That is the point — but it means a
//! wrong key burned at manufacture bricks the unit's ability to ever accept an
//! update, so the value must be verified before the protect bit is set.
//!
//! Use `tools/provision-efuse-key.sh`, which does that in the right order:
//! refuses a non-empty block, burns, reads back and compares, and only then
//! sets the protect bit. If the read-back fails it deliberately leaves the
//! block writable rather than sealing a wrong value — a writable block is
//! refused by `key::resolve`, which is recoverable, whereas a sealed wrong key
//! is not.
//!
//! ```text
//! ./tools/provision-efuse-key.sh --port /dev/cu.usbserial-XXXX --check
//! ./tools/provision-efuse-key.sh --port /dev/cu.usbserial-XXXX --burn
//! ```

use super::key::{EfuseKeySlot, PUBLIC_KEY_LEN};
use super::signature::{Verifier, SIGNATURE_LEN};

/// Ed25519 verification using a pure-Rust implementation.
///
/// Deliberately not ESP-IDF's mbedtls: mbedtls builds do not reliably enable
/// Ed25519, and discovering that at flash time on a device with no update path
/// is the worst moment to find out. A pure-Rust verifier depends only on the
/// Rust toolchain.
pub struct DeviceVerifier;

impl Verifier for DeviceVerifier {
    fn verify(
        &self,
        message: &[u8],
        signature: &[u8; SIGNATURE_LEN],
        public_key: &[u8; PUBLIC_KEY_LEN],
    ) -> bool {
        use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};

        // A malformed key is a refusal, not a panic. This runs on a truck.
        let Ok(key) = VerifyingKey::from_bytes(public_key) else {
            return false;
        };
        key.verify(message, &Signature::from_bytes(signature))
            .is_ok()
    }
}

#[cfg(target_os = "espidf")]
mod efuse {
    use super::{EfuseKeySlot, PUBLIC_KEY_LEN};
    use esp_idf_svc::sys::{
        esp_efuse_block_t, esp_efuse_desc_t, esp_efuse_read_block, esp_efuse_read_field_bit, ESP_OK,
    };

    // The general-purpose user block is BLOCK3 on both parts, but ESP-IDF names
    // it differently per chip, so the symbols cannot be shared. Selecting them
    // by cfg makes an unsupported target a compile error naming the block —
    // rather than a build that succeeds and burns the wrong fuse, which is
    // irreversible.
    #[cfg(esp32)]
    use esp_idf_svc::sys::{
        esp_efuse_block_t_EFUSE_BLK3 as USER_BLOCK, ESP_EFUSE_WR_DIS_BLOCK3 as WR_DIS_USER_BLOCK,
    };

    #[cfg(esp32s3)]
    use esp_idf_svc::sys::{
        esp_efuse_block_t_EFUSE_BLK_USER_DATA as USER_BLOCK,
        ESP_EFUSE_WR_DIS_BLOCK_USR_DATA as WR_DIS_USER_BLOCK,
    };

    #[cfg(not(any(esp32, esp32s3)))]
    compile_error!(
        "OTA signing-key eFuse block is only mapped for esp32 and esp32s3. \
         Add the correct user-data block and its write-disable field for this \
         chip before building — burning the wrong block cannot be undone."
    );

    /// The block holding the firmware signing public key. See the module docs
    /// before changing this — the wrong block collides with Secure Boot.
    ///
    /// Note on the descriptor symbol: bindgen renders the C flexible array
    /// `extern const esp_efuse_desc_t* ESP_EFUSE_WR_DIS_<block>[]` as
    /// `[*const esp_efuse_desc_t; 0]`. The zero is an unknown length, not an
    /// empty field — the address is still correct, which is why the pointer is
    /// taken rather than the (zero) contents.
    pub const EFUSE_PUBLIC_KEY_BLOCK: esp_efuse_block_t = USER_BLOCK;

    /// Read the key and its write-protect state.
    ///
    /// Returns `None` when the block cannot be read at all. `None` and "block
    /// of zeros" are different: the first is a hardware or API failure, the
    /// second is an unprovisioned unit, and only the second is a normal state.
    pub fn read_signing_key() -> Option<EfuseKeySlot> {
        let mut key = [0u8; PUBLIC_KEY_LEN];

        // Offsets and lengths are in bits.
        let status = unsafe {
            esp_efuse_read_block(
                EFUSE_PUBLIC_KEY_BLOCK,
                key.as_mut_ptr() as *mut core::ffi::c_void,
                0,
                (PUBLIC_KEY_LEN * 8) as usize,
            )
        };
        if status != ESP_OK {
            return None;
        }

        Some(EfuseKeySlot {
            key,
            write_protected: is_write_protected(),
        })
    }

    /// Whether the key block's write-protect bit is burned.
    ///
    /// `esp_efuse_read_field_bit` returns the bit directly rather than an error
    /// code, so there is no separate failure to distinguish: an unburned bit and
    /// an unreadable one both come back false, which is the safe direction.
    /// `key::resolve` refuses a key whose block is not protected, so failing
    /// this way blocks updates rather than accepting an attacker-writable key.
    fn is_write_protected() -> bool {
        // `static mut` from bindgen: take a raw pointer rather than a reference,
        // which is undefined behaviour on a mutable static.
        let field = core::ptr::addr_of_mut!(WR_DIS_USER_BLOCK) as *mut *const esp_efuse_desc_t;
        unsafe { esp_efuse_read_field_bit(field) }
    }
}

/// Resolve the trusted key and verify a manifest in one step.
///
/// A single entry point on purpose. Splitting "which key do we trust" from "is
/// this signature valid" across two call sites is how an update path ends up
/// verifying against a key it should have refused — the writable-eFuse case in
/// particular looks like a successful key read.
///
/// Any error means do not install. There is no partial success here.
pub fn verify_update(
    document: &str,
    manifest: &super::manifest::Manifest,
) -> Result<super::key::KeySource, UpdateAuthError> {
    let trusted = super::key::resolve(efuse_signing_key()).map_err(UpdateAuthError::Key)?;
    super::signature::verify_manifest(&DeviceVerifier, document, manifest, &trusted.key)
        .map_err(UpdateAuthError::Signature)?;
    Ok(trusted.source)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAuthError {
    /// No trustworthy key. Distinct from a bad signature: this is a
    /// provisioning problem, not an attack, and the fix is different.
    Key(super::key::KeyError),
    /// A key was available and the manifest failed against it.
    Signature(super::signature::SignatureError),
}

/// Read the eFuse signing key, if this build runs on a device.
///
/// On the host this is always `None`, which makes `key::resolve` fall through
/// to the build-time key — the correct behaviour for a host test binary.
pub fn efuse_signing_key() -> Option<EfuseKeySlot> {
    #[cfg(target_os = "espidf")]
    {
        efuse::read_signing_key()
    }
    #[cfg(not(target_os = "espidf"))]
    {
        None
    }
}
