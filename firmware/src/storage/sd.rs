//! SD card bring-up over SPI, mounted as a VFS path so `std::fs` works.
//!
//! Chain: `SpiDriver` → `SdSpiHostDriver` → `SdCardDriver` → `Fatfs` →
//! `MountedFatfs("/sd")`. The returned `SdMount` owns the whole stack; keep it
//! alive for as long as you want the card mounted (dropping it unmounts).
//!
//! NOTE: untested on hardware. Verify the SD wiring (config::SD_* pins) and, if
//! the card won't init, lower the SPI clock. Gated behind ENABLE_SD_LOGGING, so
//! the firmware builds and runs with no SD attached.

use esp_idf_svc::fs::fatfs::config::FormatConfiguration;
use esp_idf_svc::fs::fatfs::Fatfs;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::sd::spi::SdSpiHostDriver;
use esp_idf_svc::hal::sd::{SdCardConfiguration, SdCardDriver};
use esp_idf_svc::hal::spi::config::DriverConfig;
use esp_idf_svc::hal::spi::{Dma, SpiDriver, SPI2};
use esp_idf_svc::io::vfs::MountedFatfs;
use log::{info, warn};
use std::io::{Read, Write};
use std::path::Path;

use super::files::SD_ROOT;
use crate::config;
use crate::error::EldError;

/// Max simultaneously open files on the FAT mount.
const MAX_OPEN_FILES: usize = 5;
const FORMAT_WORKBUF_BYTES: usize = 4096;
/// SPI DMA max transfer size. Must cover a 512-byte SD sector (plus headroom);
/// without DMA the host caps transfers too small and sector reads fail with
/// "txdata transfer > host maximum".
const SPI_MAX_TRANSFER_BYTES: usize = 4096;

type Host = SdSpiHostDriver<'static, SpiDriver<'static>>;
type Card = SdCardDriver<Host>;
type Fs = Fatfs<Card>;

/// A live SD mount. Owns the full driver stack; drop to unmount.
pub type SdMount = MountedFatfs<Fs>;

#[derive(Debug, Clone, Copy)]
pub struct Capacity {
    pub total_bytes: u64,
}

/// Bring up the SD card and mount it at `/sd`.
///
/// Peripherals (SPI2 + the configured GPIOs) are passed in by `App::boot`, which
/// owns the single `Peripherals::take()` and distributes parts to each engine.
pub fn mount(
    spi2: SPI2<'static>,
    sclk: AnyIOPin<'static>,
    mosi: AnyIOPin<'static>,
    miso: AnyIOPin<'static>,
    cs: AnyIOPin<'static>,
) -> Result<(SdMount, Capacity), EldError> {
    let spi = SpiDriver::new(
        spi2,
        sclk,
        mosi,
        Some(miso),
        &DriverConfig::new().dma(Dma::Auto(SPI_MAX_TRANSFER_BYTES)),
    )
    .map_err(|_| EldError::Storage("spi init failed"))?;

    let host = SdSpiHostDriver::new(
        spi,
        Some(cs),
        Option::<AnyIOPin>::None, // card-detect
        Option::<AnyIOPin>::None, // write-protect
        Option::<AnyIOPin>::None, // interrupt
        None,                     // wp_active_high
    )
    .map_err(|_| EldError::Storage("sdspi host init failed"))?;

    let mut sd_config = SdCardConfiguration::default();
    sd_config.speed_khz = config::SD_SPI_SPEED_KHZ;
    sd_config.command_timeout_ms = 1_000;

    let card = SdCardDriver::new_spi(host, &sd_config).map_err(|e| {
        warn!(
            "storage: sd card init failed at {} kHz ({e}); check wiring/power/CS/card detect",
            config::SD_SPI_SPEED_KHZ
        );
        EldError::Storage("sd card init failed")
    })?;
    let capacity = Capacity {
        total_bytes: (card.card().csd.capacity as u64)
            .saturating_mul(card.card().csd.sector_size as u64),
    };

    let mut fatfs = Fatfs::new_sdcard(0, card).map_err(|e| {
        warn!("storage: fatfs disk registration failed ({e})");
        EldError::Storage("fatfs init failed")
    })?;

    if config::SD_FORMAT_ON_BOOT {
        warn!("storage: formatting SD because SD_FORMAT_ON_BOOT=true");
        format_sd(&mut fatfs)?;
        info!("storage: SD formatted on boot");
    } else if fatfs.mount().is_err() {
        if !config::SD_FORMAT_ON_FIRST_USE {
            return Err(EldError::Storage("fat mount failed"));
        }

        warn!("storage: FAT mount failed; formatting SD because SD_FORMAT_ON_FIRST_USE=true");
        format_sd(&mut fatfs)?;

        fatfs.mount().map_err(|e| {
            warn!("storage: FAT mount after format failed ({e})");
            EldError::Storage("fat mount failed after format")
        })?;
        info!("storage: SD formatted for first use");
    }

    let mounted = MountedFatfs::mount(fatfs, SD_ROOT, MAX_OPEN_FILES).map_err(|e| {
        warn!("storage: VFS FAT mount at {SD_ROOT} failed ({e})");
        EldError::Storage("fat vfs mount failed")
    })?;
    Ok((mounted, capacity))
}

fn format_sd(fatfs: &mut Fs) -> Result<(), EldError> {
    let mut workbuf = [0u8; FORMAT_WORKBUF_BYTES];
    fatfs
        .format(&FormatConfiguration::default(), &mut workbuf)
        .map_err(|e| {
            warn!("storage: SD format failed ({e})");
            EldError::Storage("sd format failed")
        })
}

/// Non-destructive boot probe: write, flush, read back, compare, then remove a
/// small firmware-owned file. Directory creation alone does not prove that
/// data survives a complete filesystem round trip.
pub fn verify_read_write(root: &Path) -> std::io::Result<()> {
    const PROBE: &[u8] = b"eld-sd-health-v1\n";
    let path = root.join("eldcheck.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)?;
        file.write_all(PROBE)?;
        file.flush()?;
        drop(file);

        let mut read_back = Vec::with_capacity(PROBE.len());
        std::fs::File::open(&path)?.read_to_end(&mut read_back)?;
        if read_back != PROBE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SD read-back mismatch",
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    std::fs::remove_file(path)
}
