use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;
use susi_client::{LicenseClient, LicenseStatus};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Default)]
pub struct LicenseInfo {
    pub valid: bool,
    pub status: String,
    pub customer: String,
    pub product: String,
    pub features: Vec<String>,
    pub expires: Option<String>,
    pub lease_expires: Option<String>,
    pub machine_code: String,
    pub license_key: String,
    pub error: String,
}

pub struct Crypto {
    features: Vec<String>,
    last_info: LicenseInfo,
}

/// Configuration extracted from the "LicenseInfo" section of the config JSON.
struct LicenseConfig {
    license_file: PathBuf,
    license_key: String,
    server_url: Option<String>,
}

impl LicenseConfig {
    fn from_json(value: &Value) -> Self {
        let license_file = value
            .get("LicenseFile")
            .and_then(|v| v.as_str())
            .unwrap_or("license.json")
            .into();

        let license_key = value
            .get("LicenseKey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let server_url = value
            .get("ServerUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Self {
            license_file,
            license_key,
            server_url,
        }
    }
}

/// Default embedded public key.
/// Replace this with your actual public key generated via `license-admin keygen`.
/// When empty, license check is skipped (development mode).
const DEFAULT_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEAuCdhRBVGSMFb93EnsA2P
5jyOWGp5DAM4TuizWvV2WDFMapwHpCHSbyQSNnbmeaMXi1bwjTgICAgfCw75t31C
oUcK5dJStuzvD991gZuJY8TzzbcSOU4DH8O1h7zU+F4IUZTU7VeYVN0hxskjl3E0
IaoQWy3NgMkRKIfcXaEdpkaTYGtmeY2Aw7Z/w+/7d8UVhbOArHvSlpuMxlNl39wN
QQSRJrhpV0e+A0Z5JVBx/R2c8Sd6FIPCTQYmUbdHvGgbc1p3swwFNF5wPkF5soWE
9kJm9MiShSDhD4JYpAlCfcOl05Xwtt0vQ4IHRoqPjcMo2iU6QMVyij/I4QO17ZOF
rpnmEWE7pMvpJ0nG0P7/BkRgouVp64jFNiFTro9X/DzzRjapJ4Zuf3jSOazeqjhr
ry2MFh81tvF7DC5ysVi1YFx8WOW+mtm7ca1BGnrgUIfRDdgVn3ChCpvz0uJnJXyH
WnsimjIqnzoX0UOY4BKPiC3MhN85pD1h1LFcpJaAJTV8O7voGcZz1N+tNFAzCduL
vh7kD+A2jBjtz6sQSljFNcvNYU+84xUmHXHG8afjFkMhLMwmGLhmgJWZuERzMNR3
dOwLE2chvhUsCYOhdMfsCfKYDqDDVwx6zNydRRNnn5w/2XTRRJhAoJSWLvfOe0FB
R9n/itr/sWjM9zI2xh9qmvcCAwEAAQ==
-----END PUBLIC KEY-----";

fn format_dt(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn machine_code_str() -> String {
    LicenseClient::get_machine_code().unwrap_or_default()
}

fn status_to_info(status: &LicenseStatus, machine_code: &str) -> LicenseInfo {
    match status {
        LicenseStatus::Valid { payload } => LicenseInfo {
            valid: true,
            status: "valid".into(),
            customer: payload.customer.clone(),
            product: payload.product.clone(),
            features: payload.features.clone(),
            expires: payload.expires.as_ref().map(format_dt),
            lease_expires: payload.lease_expires.as_ref().map(format_dt),
            machine_code: machine_code.into(),
            license_key: payload.license_key.clone(),
            error: String::new(),
        },
        LicenseStatus::ValidGracePeriod { payload, lease_expired_at } => LicenseInfo {
            valid: true,
            status: "grace_period".into(),
            customer: payload.customer.clone(),
            product: payload.product.clone(),
            features: payload.features.clone(),
            expires: payload.expires.as_ref().map(format_dt),
            lease_expires: Some(format_dt(lease_expired_at)),
            machine_code: machine_code.into(),
            license_key: payload.license_key.clone(),
            error: format!("Lease expired at {}, renew ASAP", lease_expired_at.format("%Y-%m-%d %H:%M")),
        },
        LicenseStatus::Expired { expired_at } => LicenseInfo {
            valid: false,
            status: "expired".into(),
            machine_code: machine_code.into(),
            error: format!("Expired on {}", expired_at.format("%Y-%m-%d")),
            ..Default::default()
        },
        LicenseStatus::LeaseExpired { lease_expired_at } => LicenseInfo {
            valid: false,
            status: "lease_expired".into(),
            machine_code: machine_code.into(),
            error: format!("Lease expired at {}", lease_expired_at.format("%Y-%m-%d %H:%M")),
            ..Default::default()
        },
        LicenseStatus::InvalidMachine { expected: _, actual } => LicenseInfo {
            valid: false,
            status: "invalid_machine".into(),
            machine_code: actual.clone(),
            error: "License not valid for this machine".into(),
            ..Default::default()
        },
        LicenseStatus::InvalidSignature => LicenseInfo {
            valid: false,
            status: "invalid_signature".into(),
            machine_code: machine_code.into(),
            error: "License file has an invalid signature".into(),
            ..Default::default()
        },
        LicenseStatus::TokenNotFound => LicenseInfo {
            valid: false,
            status: "token_not_found".into(),
            machine_code: machine_code.into(),
            error: "No valid USB hardware token found".into(),
            ..Default::default()
        },
        LicenseStatus::FileNotFound(path) => LicenseInfo {
            valid: false,
            status: "file_not_found".into(),
            machine_code: machine_code.into(),
            error: format!("License file not found: {}", path),
            ..Default::default()
        },
        LicenseStatus::Error(msg) => LicenseInfo {
            valid: false,
            status: "error".into(),
            machine_code: machine_code.into(),
            error: msg.clone(),
            ..Default::default()
        },
    }
}

impl Crypto {
    pub fn new() -> Self {
        Self {
            features: Vec::new(),
            last_info: LicenseInfo::default(),
        }
    }

    fn make_client(server_url: Option<&str>) -> Result<LicenseClient, String> {
        if DEFAULT_PUBLIC_KEY.is_empty() {
            return Err("no_key".into());
        }
        let client = match server_url {
            Some(url) => LicenseClient::with_server(DEFAULT_PUBLIC_KEY, url.to_owned()),
            None => LicenseClient::new(DEFAULT_PUBLIC_KEY),
        };
        client.map_err(|e| format!("Failed to create license client: {}", e))
    }

    fn apply_status(&mut self, status: &LicenseStatus) -> LicenseInfo {
        let mc = machine_code_str();
        let info = status_to_info(status, &mc);
        if info.valid {
            self.features = info.features.clone();
        } else {
            self.features.clear();
        }
        self.last_info = info.clone();
        info
    }

    pub fn check_license(&mut self, json_license_info: &str) -> bool {
        let value: Value = match serde_json::from_str(json_license_info) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Could not parse license info JSON: {}", e);
                return false;
            }
        };

        let config = LicenseConfig::from_json(&value);

        if DEFAULT_PUBLIC_KEY.is_empty() {
            log::warn!("No public key configured, skipping license check");
            return true;
        }

        let client = match Self::make_client(config.server_url.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                log::error!("{}", e);
                return false;
            }
        };

        let status = if !config.license_key.is_empty() && config.server_url.is_some() {
            client.verify_and_refresh(&config.license_file, &config.license_key)
        } else {
            client.verify_file(&config.license_file)
        };

        let info = self.apply_status(&status);
        log_license_status(&info);
        info.valid
    }

    pub fn check_license_file(&mut self, license_file: &str) -> LicenseInfo {
        if DEFAULT_PUBLIC_KEY.is_empty() {
            log::warn!("No public key configured, skipping license check");
            return LicenseInfo { valid: true, status: "valid".into(), ..Default::default() };
        }
        let client = match Self::make_client(None) {
            Ok(c) => c,
            Err(e) => return LicenseInfo { status: "error".into(), error: e, ..Default::default() },
        };
        let status = client.verify_file(license_file.as_ref());
        let info = self.apply_status(&status);
        log_license_status(&info);
        info
    }

    pub fn check_license_server(&mut self, license_file: &str, license_key: &str, server_url: &str) -> LicenseInfo {
        if DEFAULT_PUBLIC_KEY.is_empty() {
            log::warn!("No public key configured, skipping license check");
            return LicenseInfo { valid: true, status: "valid".into(), ..Default::default() };
        }
        let client = match Self::make_client(Some(server_url)) {
            Ok(c) => c,
            Err(e) => return LicenseInfo { status: "error".into(), error: e, ..Default::default() },
        };
        let status = client.verify_and_refresh(license_file.as_ref(), license_key);
        let info = self.apply_status(&status);
        log_license_status(&info);
        info
    }

    pub fn check_license_token(&mut self) -> LicenseInfo {
        if DEFAULT_PUBLIC_KEY.is_empty() {
            log::warn!("No public key configured, skipping license check");
            return LicenseInfo { valid: true, status: "valid".into(), ..Default::default() };
        }
        let client = match Self::make_client(None) {
            Ok(c) => c,
            Err(e) => return LicenseInfo { status: "error".into(), error: e, ..Default::default() },
        };
        let status = client.verify_token();
        let info = self.apply_status(&status);
        log_license_status(&info);
        info
    }

    pub fn get_machine_code() -> String {
        machine_code_str()
    }

    pub fn last_info(&self) -> &LicenseInfo {
        &self.last_info
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

fn log_license_status(info: &LicenseInfo) {
    if info.valid {
        log::info!(
            "License valid (product: {}, customer: {}, features: {})",
            info.product,
            info.customer,
            if info.features.is_empty() { "-".into() } else { info.features.join(", ") },
        );
    } else if !info.error.is_empty() {
        log::error!("License check failed: {}", info.error);
    }
}

impl Default for Crypto {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_license_no_file_fails() {
        // With public key configured but no license file, check should fail
        let mut c = Crypto::new();
        assert!(!c.check_license("{}"));
    }

    #[test]
    fn check_license_invalid_json() {
        let mut c = Crypto::new();
        assert!(!c.check_license("not json"));
    }

    #[test]
    fn check_license_file_not_found() {
        let mut c = Crypto::new();
        let info = serde_json::json!({
            "LicenseFile": "/nonexistent/license.json",
        });
        assert!(!c.check_license(&info.to_string()));
        assert_eq!(c.last_info().status, "file_not_found");
    }

    /// Test that LicenseClient directly verifies a valid signed license file.
    /// (The Crypto wrapper uses the compiled-in DEFAULT_PUBLIC_KEY which is empty
    /// in tests, so we test the underlying client directly here.)
    #[test]
    fn license_client_verifies_valid_file() {
        use chrono::{Duration, Utc};

        let (private, public) = susi_core::generate_keypair(2048).unwrap();
        let pub_pem = susi_core::crypto::public_key_to_pem(&public).unwrap();

        let payload = susi_core::LicensePayload {
            id: "test".to_string(),
            product: "FusionHub".to_string(),
            customer: "Test".to_string(),
            license_key: "AAAA-BBBB-CCCC-DDDD".to_string(),
            created: Utc::now(),
            expires: Some(Utc::now() + Duration::days(365)),
            features: vec!["full_fusion".to_string(), "recorder".to_string()],
            machine_codes: vec![],
            lease_expires: None,
        };

        let signed = susi_core::sign_license(&private, &payload).unwrap();
        let tmp = std::env::temp_dir().join("test_crypto_license.json");
        let json = serde_json::to_string_pretty(&signed).unwrap();
        std::fs::write(&tmp, &json).unwrap();

        let client = LicenseClient::new(&pub_pem).unwrap();
        let status = client.verify_file(&tmp);
        assert!(matches!(status, LicenseStatus::Valid { .. }));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn default_crypto() {
        let c = Crypto::default();
        assert!(c.features().is_empty());
    }
}
