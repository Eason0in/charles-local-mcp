use std::{collections::BTreeMap, fs, net::IpAddr, path::Path};

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfilesFile {
    pub schema_version: u32,
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub source_host: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub destination_url: Option<String>,
    #[serde(default)]
    pub ssl_hosts: Vec<String>,
    #[serde(default)]
    pub verification_url: Option<String>,
}

impl ProfilesFile {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("unable to read profiles file {}: {error}", path.display()))?;
        let profiles: Self = toml::from_str(&text)
            .map_err(|error| format!("invalid profiles file {}: {error}", path.display()))?;
        profiles.validate()?;
        Ok(profiles)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported profile schemaVersion {}; expected 1",
                self.schema_version
            ));
        }
        if self.profiles.is_empty() {
            return Err("profiles must contain at least one profile".into());
        }
        for (name, profile) in &self.profiles {
            validate_profile_name(name)?;
            profile
                .validate()
                .map_err(|error| format!("profile {name}: {error}"))?;
        }
        Ok(())
    }
}

impl Profile {
    pub fn validate(&self) -> Result<(), String> {
        validate_dns_host(&self.source_host)?;
        if let Some(path) = self.source_path.as_deref() {
            if !path.starts_with('/') || path.contains("..") {
                return Err("sourcePath must be an absolute path pattern without '..'".into());
            }
        }
        if self.source_path.is_some() && self.destination_url.is_none() {
            return Err("sourcePath requires destinationUrl".into());
        }
        if let Some(raw) = self.destination_url.as_deref() {
            let destination =
                Url::parse(raw).map_err(|error| format!("invalid destinationUrl: {error}"))?;
            if destination.scheme() != "http" && destination.scheme() != "https" {
                return Err("destinationUrl must use http or https".into());
            }
            if destination.username() != "" || destination.password().is_some() {
                return Err("destinationUrl must not contain credentials".into());
            }
            if destination.query().is_some() || destination.fragment().is_some() {
                return Err("destinationUrl must not contain a query or fragment".into());
            }
            let host = destination
                .host_str()
                .ok_or_else(|| "destinationUrl must contain a host".to_owned())?;
            if !is_loopback_host(host) {
                return Err(
                    "destinationUrl host must be loopback (localhost, 127.0.0.0/8, or ::1)".into(),
                );
            }
            if destination.port_or_known_default().is_none() {
                return Err("destinationUrl must have a known or explicit port".into());
            }
        }
        for host in std::iter::once(&self.source_host).chain(self.ssl_hosts.iter()) {
            validate_dns_host(host)?;
            if host.contains('*') {
                return Err("wildcard hosts are not allowed".into());
            }
        }
        if let Some(raw) = &self.verification_url {
            let verification =
                Url::parse(raw).map_err(|error| format!("invalid verificationUrl: {error}"))?;
            if verification.scheme() != "https" {
                return Err("verificationUrl must use https".into());
            }
            if verification.host_str() != Some(self.source_host.as_str()) {
                return Err("verificationUrl host must exactly match sourceHost".into());
            }
        }
        Ok(())
    }
}

fn validate_profile_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("invalid profile name {name:?}"));
    }
    Ok(())
}

fn validate_dns_host(host: &str) -> Result<(), String> {
    if host.is_empty()
        || host.len() > 253
        || host.contains('/')
        || host.contains(':')
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(format!("invalid exact DNS host {host:?}"));
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(destination: &str) -> Profile {
        Profile {
            source_host: "app.example.com".into(),
            source_path: None,
            destination_url: Some(destination.into()),
            ssl_hosts: vec!["api.example.com".into()],
            verification_url: Some("https://app.example.com/health".into()),
        }
    }

    #[test]
    fn accepts_loopback_destination_and_exact_hosts() {
        for destination in [
            "http://localhost:8080",
            "http://127.0.0.2:8080",
            "https://[::1]:8443",
        ] {
            profile(destination).validate().unwrap();
        }
    }

    #[test]
    fn rejects_non_loopback_or_mismatched_verification_url() {
        assert!(profile("http://example.com:8080").validate().is_err());
        let mut value = profile("http://127.0.0.1:8080");
        value.verification_url = Some("https://other.example.com/health".into());
        assert!(value.validate().is_err());
    }

    #[test]
    fn accepts_proxy_only_profile_and_rejects_path_without_mapping() {
        let mut value = profile("http://127.0.0.1:8080");
        value.destination_url = None;
        value.validate().unwrap();

        value.source_path = Some("/mapped*".into());
        assert!(value.validate().is_err());
    }
}
