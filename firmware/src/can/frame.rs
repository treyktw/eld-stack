//! Owned, driver-independent CAN frame.
//!
//! The HAL's `Frame` borrows an internal IDF struct; we copy the fields we care
//! about into a plain value so the rest of the firmware never depends on the
//! HAL type and frames can be queued/logged freely.

use core::fmt;

use esp_idf_svc::hal::can::{Flags, Frame as HalFrame};

use super::errors::CanError;

#[derive(Debug, Clone, Copy)]
pub struct CanFrame {
    /// Raw arbitration ID (11-bit standard or 29-bit extended).
    pub id: u32,
    pub extended: bool,
    pub remote: bool,
    pub dlc: u8,
    pub data: [u8; 8],
}

impl CanFrame {
    pub fn new(id: u32, extended: bool, remote: bool, payload: &[u8]) -> Result<Self, CanError> {
        if payload.len() > 8 {
            return Err(CanError::BadFrame);
        }
        if extended && id > 0x1FFF_FFFF {
            return Err(CanError::BadFrame);
        }
        if !extended && id > 0x7FF {
            return Err(CanError::BadFrame);
        }

        let mut data = [0u8; 8];
        data[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            id,
            extended,
            remote,
            dlc: payload.len() as u8,
            data,
        })
    }

    pub fn extended(id: u32, payload: &[u8]) -> Result<Self, CanError> {
        Self::new(id, true, false, payload)
    }

    /// An 11-bit standard frame, as OBD-II uses.
    ///
    /// Rejects an id that will not fit 11 bits rather than truncating it — a
    /// silently masked identifier addresses a different ECU than intended.
    pub fn standard(id: u16, payload: &[u8]) -> Result<Self, CanError> {
        if id > 0x7FF {
            return Err(CanError::BadFrame);
        }
        Self::new(id as u32, false, false, payload)
    }

    pub fn from_hal(f: &HalFrame) -> Self {
        let mut data = [0u8; 8];
        let src = f.data();
        data[..src.len()].copy_from_slice(src);
        Self {
            id: f.identifier(),
            extended: f.is_extended(),
            remote: f.is_remote_frame(),
            dlc: f.dlc() as u8,
            data,
        }
    }

    pub fn to_hal(self) -> Result<HalFrame, CanError> {
        if self.remote {
            return HalFrame::new_remote(self.id, self.extended, self.dlc as usize)
                .ok_or(CanError::BadFrame);
        }

        let flags = if self.extended {
            [Flags::Extended].into()
        } else {
            [Flags::None].into()
        };
        HalFrame::new(self.id, flags, self.payload()).ok_or(CanError::BadFrame)
    }

    /// Payload bytes actually present (`dlc` of them).
    pub fn payload(&self) -> &[u8] {
        &self.data[..self.dlc as usize]
    }

    pub fn is_j1939_candidate(&self) -> bool {
        self.extended && !self.remote && self.id <= 0x1FFF_FFFF
    }

    pub fn log_id(&self) -> FrameId<'_> {
        FrameId(self)
    }
}

pub struct FrameId<'a>(&'a CanFrame);

impl fmt::Display for FrameId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.extended {
            write!(f, "0x{:08X}", self.0.id)?;
        } else {
            write!(f, "0x{:03X}", self.0.id)?;
        }

        Ok(())
    }
}
