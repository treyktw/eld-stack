//! WiFi STA link + HTTPS upload.
//!
//! NOTE: untested on hardware. Gated behind ENABLE_WIFI_UPLOAD, so the firmware
//! builds and runs without WiFi. Set config::WIFI_SSID/PASS/UPLOAD_URL before
//! enabling. Takes the modem peripheral + system event loop + NVS internally.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::http::Method;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

use crate::config;
use crate::error::EldError;

pub struct WifiLink {
    wifi: BlockingWifi<EspWifi<'static>>,
}

impl WifiLink {
    /// Bring up the STA interface and start associating with the configured AP.
    /// Peripherals (modem + event loop + NVS) come from `App::boot`.
    pub fn connect(
        modem: Modem<'static>,
        sysloop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
    ) -> Result<Self, EldError> {
        if config::WIFI_SSID.is_empty() {
            return Err(EldError::Comms("wifi ssid empty"));
        }

        let wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs))
            .map_err(|_| EldError::Comms("wifi init"))?;
        let mut wifi =
            BlockingWifi::wrap(wifi, sysloop).map_err(|_| EldError::Comms("wifi wrapper"))?;

        let cfg = Configuration::Client(ClientConfiguration {
            ssid: config::WIFI_SSID
                .try_into()
                .map_err(|_| EldError::Comms("ssid too long"))?,
            password: config::WIFI_PASS
                .try_into()
                .map_err(|_| EldError::Comms("password too long"))?,
            auth_method: AuthMethod::WPA2Personal,
            ..Default::default()
        });
        wifi.set_configuration(&cfg)
            .map_err(|_| EldError::Comms("set config"))?;
        wifi.start().map_err(|_| EldError::Comms("wifi start"))?;
        wifi.connect()
            .map_err(|_| EldError::Comms("wifi connect"))?;
        wifi.wait_netif_up()
            .map_err(|_| EldError::Comms("wifi dhcp"))?;

        Ok(Self { wifi })
    }

    /// True once associated and the netif has an IP.
    pub fn is_up(&self) -> bool {
        self.wifi.is_up().unwrap_or(false)
    }

    /// POST a JSON body. Returns the HTTP status on success.
    pub fn post(&mut self, url: &str, body: &[u8]) -> Result<u16, EldError> {
        let mut conn = EspHttpConnection::new(&HttpConfig::default())
            .map_err(|_| EldError::Comms("http client"))?;

        // Bearer auth when a token is configured (matches server INGEST_TOKEN).
        let bearer = format!("Bearer {}", config::UPLOAD_TOKEN);
        let mut headers: heapless::Vec<(&str, &str), 2> = heapless::Vec::new();
        let _ = headers.push(("content-type", "application/json"));
        if !config::UPLOAD_TOKEN.is_empty() {
            let _ = headers.push(("authorization", bearer.as_str()));
        }

        conn.initiate_request(Method::Post, url, &headers)
            .map_err(|_| EldError::Comms("http request"))?;
        conn.write_all(body)
            .map_err(|_| EldError::Comms("http write"))?;
        conn.initiate_response()
            .map_err(|_| EldError::Comms("http response"))?;
        let status = conn.status();

        let mut buf = [0u8; 128];
        loop {
            match conn.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => return Err(EldError::Comms("http response read")),
            }
        }

        Ok(status)
    }
}
