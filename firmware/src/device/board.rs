//! T-SIM7670G-S3-Standard board-level modem power and GNSS bring-up.
//!
//! These pins are fixed by the PCB. Keeping their drivers alive matters:
//! dropping a `PinDriver` resets the GPIO, which can put the modem back to
//! sleep or reduce the board power rail while the application is running.

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{AnyIOPin, Output, PinDriver};
use esp_idf_svc::hal::uart::UART2;
use log::{info, warn};

use crate::comms::cellular::modem::Modem;
use crate::error::EldError;

const AT_TIMEOUT_MS: u64 = 1_000;
const MODEM_BOOT_WAIT_MS: u32 = 3_000;
const MODEM_PROBE_RETRIES: usize = 10;

/// GPIO state that must remain asserted while the SIM7670G is in use.
pub struct ModemPower {
    _pwrkey: PinDriver<'static, Output>,
    _dtr: PinDriver<'static, Output>,
    _power_save: PinDriver<'static, Output>,
}

impl ModemPower {
    /// Open the AT UART, start the modem if needed, and enable its GNSS output.
    /// No SIM is required for this sequence.
    pub fn start(
        uart2: UART2<'static>,
        modem_tx: AnyIOPin<'static>,
        modem_rx: AnyIOPin<'static>,
        pwrkey_pin: AnyIOPin<'static>,
        dtr_pin: AnyIOPin<'static>,
        power_save_pin: AnyIOPin<'static>,
    ) -> Result<(Self, Modem<'static>), EldError> {
        let mut power_save = PinDriver::output(power_save_pin)
            .map_err(|_| EldError::Comms("power-save GPIO init failed"))?;
        power_save
            .set_high()
            .map_err(|_| EldError::Comms("power-save GPIO set failed"))?;

        let mut dtr = PinDriver::output(dtr_pin)
            .map_err(|_| EldError::Comms("modem DTR GPIO init failed"))?;
        dtr.set_low()
            .map_err(|_| EldError::Comms("modem DTR GPIO set failed"))?;

        let mut pwrkey = PinDriver::output(pwrkey_pin)
            .map_err(|_| EldError::Comms("modem PWRKEY GPIO init failed"))?;
        pwrkey
            .set_low()
            .map_err(|_| EldError::Comms("modem PWRKEY GPIO set failed"))?;

        let mut modem = Modem::open(uart2, modem_tx, modem_rx)?;

        // An ESP reset does not necessarily reset the cellular module. Probe
        // first so a PWRKEY pulse cannot accidentally turn an already-running
        // modem off.
        if !responds(&mut modem) {
            info!("cellular: modem silent; applying SIM7670G PWRKEY pulse");
            FreeRtos::delay_ms(100);
            pwrkey
                .set_high()
                .map_err(|_| EldError::Comms("modem PWRKEY pulse failed"))?;
            FreeRtos::delay_ms(100);
            pwrkey
                .set_low()
                .map_err(|_| EldError::Comms("modem PWRKEY pulse failed"))?;
            FreeRtos::delay_ms(MODEM_BOOT_WAIT_MS);
        }

        let mut ready = responds(&mut modem);
        for _ in 0..MODEM_PROBE_RETRIES {
            if ready {
                break;
            }
            FreeRtos::delay_ms(500);
            ready = responds(&mut modem);
        }

        if ready {
            info!("cellular: SIM7670G answered AT (SIM not required)");
            let _ = command(&mut modem, "ATE0");
            log_identity(&mut modem);
            configure_gnss(&mut modem);
        } else {
            warn!(
                "cellular: SIM7670G AT unavailable after startup window; dedicated GNSS UART will still be monitored"
            );
        }

        Ok((
            Self {
                _pwrkey: pwrkey,
                _dtr: dtr,
                _power_save: power_save,
            },
            modem,
        ))
    }
}

fn responds(modem: &mut Modem<'static>) -> bool {
    command(modem, "AT")
}

fn command(modem: &mut Modem<'static>, text: &str) -> bool {
    modem
        .transact(text, AT_TIMEOUT_MS, crate::time::now_ms)
        .map(|result| result.succeeded())
        .unwrap_or(false)
}

fn log_identity(modem: &mut Modem<'static>) {
    match modem.transact("AT+SIMCOMATI", 5_000, crate::time::now_ms) {
        Ok(result) if result.succeeded() => {
            for line in result.responses {
                info!("cellular: {line}");
            }
        }
        _ => warn!("cellular: could not read modem identity"),
    }
}

fn configure_gnss(modem: &mut Modem<'static>) {
    // Modem GPIO1 supplies the active GNSS antenna on this board.
    let commands = [
        "AT+CGDRT=1,1",
        "AT+CGSETV=1,1",
        "AT+CGNSSPWR=1",
        "AT+CGNSSMODE=15",
        "AT+CGNSSIPR=115200",
        // GGA + RMC are the sentences consumed by gps::nmea.
        "AT+CGNSSNMEA=1,0,0,0,1,0,0,0,0,0",
        // Do not mirror NMEA onto the AT command interface. Port 1 below is
        // the dedicated GNSS UART; leaving CGNSSTST enabled fills UART2 with
        // GPS sentences during the CAN scan and starves cellular commands.
        "AT+CGNSSTST=0",
        // Route NMEA to the dedicated modem GNSS UART (ESP GPIO45/48).
        // Port 0 is the command interface; enabling NMEA there starves AT
        // transactions once streaming begins. Port 1 is the hardware GNSS
        // UART exposed by this T-SIM7670G-S3 board.
        "AT+CGNSSPORTSWITCH=1,1",
    ];

    let mut all_ok = true;
    for text in commands {
        if !command(modem, text) {
            warn!("gps: GNSS setup command failed: {text}");
            all_ok = false;
        }
    }
    if all_ok {
        info!("gps: SIM7670G GNSS and active antenna enabled");
    }
}
