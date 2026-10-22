//! Device identity: stable per-unit ID, firmware version, board.
//!
//! The ID used to be the compile-time string `"eld-prototype-001"`, so every
//! unit built from this firmware shipped with the same identity. The ingest
//! path keys records and deduplication on `device_id`, so two units in the
//! field would have collided outright — and nothing could tell a real device
//! from a clone.
//!
//! The ID is now derived from the chip's factory eFuse MAC, which is unique per
//! ESP32 and cannot be changed. That gives every unit a distinct identity with
//! no provisioning step, and the same value can be read at manufacture to
//! register the unit against a carrier and truck.
//!
//! Note what this is *not*: a MAC is unique but public and unauthenticated. It
//! identifies a device, it does not prove one. Authenticating a device to the
//! server still needs a per-unit key (eFuse + secure boot, or a secure
//! element).

/// Render a factory MAC as the canonical device id, e.g. `eld-a1b2c3d4e5f6`.
///
/// Kept pure and separate from the FFI so it can be unit-tested on the host.
/// Sixteen characters: the BLE response lines are close to the iOS ATT MTU, so
/// the id deliberately stays short.
pub fn format_device_id(mac: [u8; 6]) -> String {
    let mut out = String::with_capacity(16);
    out.push_str("eld-");
    for byte in mac {
        out.push(nibble(byte >> 4));
        out.push(nibble(byte & 0x0f));
    }
    out
}

fn nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + (value - 10)) as char,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DeviceInfo {
    pub device_id: &'static str,
    pub firmware_version: &'static str,
    pub board: &'static str,
}

#[cfg(target_os = "espidf")]
fn efuse_mac() -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    // SAFETY: FFI write into a six-byte buffer, which is the documented size.
    let err = unsafe { esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    if err == esp_idf_svc::sys::ESP_OK {
        Some(mac)
    } else {
        None
    }
}

impl DeviceInfo {
    #[cfg(target_os = "espidf")]
    pub fn new() -> Self {
        let device_id: &'static str = match efuse_mac() {
            // Leaked deliberately: this is read once at boot and lives for the
            // process, and keeping `&'static str` avoids threading a lifetime
            // through every view and upload envelope that carries the id.
            Some(mac) => Box::leak(format_device_id(mac).into_boxed_str()),
            // A chip that cannot report its own MAC is a hardware fault. Use an
            // obviously-wrong id so it is caught in bring-up rather than
            // silently shipping duplicates again.
            None => "eld-unknown-mac",
        };

        Self {
            device_id,
            firmware_version: env!("CARGO_PKG_VERSION"),
            board: "lilygo-t-sim7670g-s3-standard",
        }
    }

    #[cfg(not(target_os = "espidf"))]
    pub fn new() -> Self {
        Self {
            device_id: "eld-host-test",
            firmware_version: env!("CARGO_PKG_VERSION"),
            board: "host",
        }
    }

    /// True when the id came from the chip rather than a fallback.
    pub fn is_derived(&self) -> bool {
        self.device_id.starts_with("eld-") && self.device_id.len() == 16
    }
}

impl Default for DeviceInfo {
    fn default() -> Self {
        Self::new()
    }
}
