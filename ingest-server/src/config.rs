//! Server configuration from environment.

use std::collections::HashMap;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    /// Device ID -> unique bearer credential. Ingest refuses to start without
    /// at least one credential so production can never fail open.
    pub device_tokens: HashMap<String, String>,
    /// Optional downstream webhook URL. Postgres remains authoritative;
    /// accepted events wait in the durable outbox until this endpoint accepts
    /// them.
    pub pipeline_url: Option<String>,
    pub pipeline_token: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL not set"))?;
        let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
        let raw_tokens = std::env::var("INGEST_DEVICE_TOKENS_JSON")
            .map_err(|_| anyhow::anyhow!("INGEST_DEVICE_TOKENS_JSON not set"))?;
        let device_tokens: HashMap<String, String> = serde_json::from_str(&raw_tokens)
            .map_err(|e| anyhow::anyhow!("INGEST_DEVICE_TOKENS_JSON is invalid: {e}"))?;
        if device_tokens.is_empty() {
            anyhow::bail!("INGEST_DEVICE_TOKENS_JSON must contain at least one device");
        }
        if device_tokens
            .iter()
            .any(|(device, token)| device.trim().is_empty() || token.len() < 32)
        {
            anyhow::bail!(
                "every ingest device must have a non-empty ID and a token of at least 32 bytes"
            );
        }
        let pipeline_url = std::env::var("PIPELINE_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty());
        let pipeline_token = std::env::var("PIPELINE_TOKEN")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if pipeline_url.is_some() != pipeline_token.is_some() {
            anyhow::bail!(
                "PIPELINE_URL and PIPELINE_TOKEN must either both be set or both be unset"
            );
        }
        if pipeline_token
            .as_ref()
            .is_some_and(|token| token.len() < 32)
        {
            anyhow::bail!("PIPELINE_TOKEN must be at least 32 bytes");
        }
        Ok(Self {
            database_url,
            bind_addr,
            device_tokens,
            pipeline_url,
            pipeline_token,
        })
    }

    /// Which device a bearer token belongs to.
    ///
    /// Inverted deliberately. Asking "does this device's token match?" requires
    /// knowing the device first, which means trusting the caller's claim about
    /// its own identity and parsing the body to find it. Asking "whose token is
    /// this?" lets the credential establish identity on its own, so the request
    /// is authenticated before a byte of the body is deserialized.
    ///
    /// Every entry is compared, with no early exit, so the time taken does not
    /// depend on which device matched or whether any did. A short-circuit here
    /// would leak whether a device id exists and roughly where in the map it
    /// sits.
    pub fn device_for_token(&self, presented: &str) -> Option<&str> {
        use subtle::ConstantTimeEq;

        let mut found: Option<&str> = None;
        for (device, token) in &self.device_tokens {
            // Length inequality is not secret — tokens are fixed-length by
            // policy — but the comparison itself must not short-circuit on
            // content.
            let matches: bool = token.len() == presented.len()
                && token.as_bytes().ct_eq(presented.as_bytes()).into();
            if matches {
                found = Some(device.as_str());
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens are 32+ bytes by policy; these are that length so the tests
    /// exercise the real comparison path rather than the length check.
    const TOKEN_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TOKEN_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn config() -> Config {
        let mut device_tokens = HashMap::new();
        device_tokens.insert("eld-aaa111".to_string(), TOKEN_A.to_string());
        device_tokens.insert("eld-bbb222".to_string(), TOKEN_B.to_string());
        Config {
            database_url: String::new(),
            bind_addr: String::new(),
            device_tokens,
            pipeline_url: None,
            pipeline_token: None,
        }
    }

    #[test]
    fn a_token_identifies_its_own_device() {
        let c = config();
        assert_eq!(c.device_for_token(TOKEN_A), Some("eld-aaa111"));
        assert_eq!(c.device_for_token(TOKEN_B), Some("eld-bbb222"));
    }

    #[test]
    fn an_unknown_token_identifies_nobody() {
        let c = config();
        assert_eq!(
            c.device_for_token("cccccccccccccccccccccccccccccccccccc"),
            None
        );
        assert_eq!(c.device_for_token(""), None);
    }

    #[test]
    fn a_prefix_of_a_valid_token_is_not_accepted() {
        // Guards against a comparison that stops at the shorter length.
        let c = config();
        assert_eq!(c.device_for_token(&TOKEN_A[..20]), None);
        assert_eq!(c.device_for_token(&TOKEN_A[..TOKEN_A.len() - 1]), None);
    }

    #[test]
    fn a_valid_token_with_extra_appended_is_not_accepted() {
        let c = config();
        let longer = format!("{TOKEN_A}x");
        assert_eq!(c.device_for_token(&longer), None);
    }

    #[test]
    fn one_devices_token_never_identifies_another() {
        // The property the whole inversion exists for: a caller cannot present
        // device B's credential and be treated as device A, whatever the body
        // claims.
        let c = config();
        assert_ne!(c.device_for_token(TOKEN_B), Some("eld-aaa111"));
    }

    #[test]
    fn a_single_character_difference_is_rejected() {
        let c = config();
        let mut near = TOKEN_A.to_string();
        near.pop();
        near.push('z');
        assert_eq!(c.device_for_token(&near), None);
    }
}
