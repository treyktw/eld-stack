//! Over-the-air firmware updates.
//!
//! Split so the decision-making is testable on the host and the parts that
//! touch flash are not entangled with it:
//!
//! - [`version`] — what "newer" means.
//! - [`manifest`] — parsing the untrusted update document.
//! - [`signature`] — proving the manifest came from us, not just that it
//!   downloaded intact.
//! - [`policy`] — whether the truck is in a state where rebooting is safe.
//!
//! The flashing itself (`esp_ota_*`) belongs in a separate module gated on
//! `target_os = "espidf"`, so that none of the above needs a device to test.
//!
//! Sequence: poll the manifest → **verify its signature** → compare versions →
//! download to the inactive OTA slot → verify size and digest → wait for a safe
//! moment → mark the new slot bootable → reboot. The bootloader's rollback
//! stays armed until the new build reports itself healthy, so a build that
//! crashes on boot returns to the previous one rather than bricking a truck.
//!
//! Signature verification comes **before** the download on purpose: it costs
//! nothing, and it means metered cellular is never spent pulling megabytes that
//! turn out to be unauthenticated.
//!
//! **Still required on the device:** `signature::Verifier` has no
//! implementation in this crate yet. The pure format is proven against real
//! Ed25519 in `firmware-tests/tests/ota_signature.rs`, but until a verifier is
//! wired here — and the public key is fixed in eFuse with Secure Boot enabled —
//! an attacker with physical access can still flash past all of this.

pub mod key;
pub mod manifest;
pub mod policy;
pub mod signature;
pub mod verifier;
pub mod version;
