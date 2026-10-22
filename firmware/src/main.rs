//! Entry point. Keep this tiny — no business logic lives here.
//!
//! init logger -> build App -> boot() -> run().

mod app;
mod can;
mod comms;
mod config;
mod device;
mod error;
mod gps;
mod j1939;
mod logging;
mod obd;
// OTA policy/signature foundations are host-tested from `firmware-tests`, but
// are deliberately not linked into the device until the flash installer and
// rollback path exist. Compiling dormant security code into production gives
// a false impression that updates are active.
mod storage;
mod telemetry;
mod time;
mod trip;
mod verifier;

use esp_idf_svc::log::EspLogger;

fn main() -> anyhow::Result<()> {
    // Required for ESP-IDF Rust runtime patches.
    esp_idf_svc::sys::link_patches();

    // Route ESP-IDF logging through the Rust `log` macros.
    EspLogger::initialize_default();

    // Box the orchestrator so its large inline state (queues, trackers) lives on
    // the heap, not the main task stack. Keeping it on the stack left too little
    // headroom for deep ESP-IDF driver-install call chains (e.g. TWAI), which
    // corrupted memory and tripped a cache-error panic.
    let mut app = Box::new(app::App::new());
    app.boot()?;
    app.run()?;

    Ok(())
}
