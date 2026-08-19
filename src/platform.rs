use std::{
    fs,
    io::{Cursor, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use quick_xml::{events::Event, Reader, Writer};

use crate::{
    model::{ProcessRecord, ProxySnapshot, ReverseSnapshot, REQUIRED_CHARLES_VERSION},
    profile::Profile,
    state::write_private,
};

pub trait Platform: Send + Sync {
    fn host_os(&self) -> String;
    fn charles_version(&self) -> Result<Option<String>, String>;
    fn source_config(&self) -> Result<PathBuf, String>;
    fn available_port(&self) -> Result<u16, String>;
    fn write_managed_config(
        &self,
        source: &Path,
        target: &Path,
        profile: &Profile,
        port: u16,
    ) -> Result<(), String>;
    fn start_charles(&self, config: &Path) -> Result<ProcessRecord, String>;
    fn stop_charles(&self, process: &ProcessRecord) -> Result<bool, String>;
    fn devices(&self) -> Result<Vec<String>, String>;
    fn ensure_reverse(&self, device: &str, host_port: u16) -> Result<ReverseSnapshot, String>;
    fn configure_proxy(&self, device: &str, port: u16) -> Result<ProxySnapshot, String>;
    fn restore_proxy(&self, snapshot: &ProxySnapshot) -> Result<bool, String>;
    fn remove_reverse(&self, snapshot: &ReverseSnapshot) -> Result<bool, String>;
    fn verify_url(&self, device: &str, url: &str) -> Result<bool, String>;
}

#[derive(Debug, Default)]
pub struct RealPlatform;

impl RealPlatform {
    fn adb(device: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new("adb")
            .arg("-s")
            .arg(device)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("unable to run adb: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "adb failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn charles_app() -> PathBuf {
        std::env::var_os("CHARLES_LOCAL_MCP_CHARLES_APP")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Applications/Charles.app"))
    }
}

impl Platform for RealPlatform {
    fn host_os(&self) -> String {
        if cfg!(target_os = "macos") {
            "macos".into()
        } else {
            std::env::consts::OS.into()
        }
    }

    fn charles_version(&self) -> Result<Option<String>, String> {
        let plist = Self::charles_app().join("Contents/Info.plist");
        if !plist.is_file() {
            return Ok(None);
        }
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
            .arg(&plist)
            .output()
            .map_err(|error| format!("unable to inspect Charles version: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "unable to read Charles version from {}",
                plist.display()
            ));
        }
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    }

    fn source_config(&self) -> Result<PathBuf, String> {
        if let Some(path) = std::env::var_os("CHARLES_LOCAL_MCP_CHARLES_CONFIG") {
            return Ok(PathBuf::from(path));
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is unavailable".to_owned())?;
        Ok(home.join("Library/Preferences/com.xk72.charles.config"))
    }

    fn available_port(&self) -> Result<u16, String> {
        for port in std::iter::once(8888).chain(8890..9000) {
            if TcpStream::connect_timeout(
                &SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(100),
            )
            .is_err()
                && TcpListener::bind(("127.0.0.1", port)).is_ok()
            {
                return Ok(port);
            }
        }
        Err("no available proxy port in 8888 or 8890-8999".into())
    }

    fn write_managed_config(
        &self,
        source: &Path,
        target: &Path,
        profile: &Profile,
        port: u16,
    ) -> Result<(), String> {
        let original = fs::read(source).map_err(|error| {
            format!(
                "unable to read Charles config {}: {error}",
                source.display()
            )
        })?;
        let managed = patch_config(&original, profile, port)?;
        write_private(target, &managed)?;
        let unchanged = fs::read(source)
            .map_err(|error| format!("unable to re-read Charles config: {error}"))?;
        if unchanged != original {
            return Err("original Charles configuration changed unexpectedly".into());
        }
        Ok(())
    }

    fn start_charles(&self, config: &Path) -> Result<ProcessRecord, String> {
        let app = Self::charles_app();
        let executable = app.join("Contents/MacOS/Charles");
        let status = Command::new("open")
            .arg("-na")
            .arg(&app)
            .arg("--args")
            .arg("-config")
            .arg(config)
            .status()
            .map_err(|error| format!("unable to launch managed Charles: {error}"))?;
        if !status.success() {
            return Err(format!("macOS failed to launch managed Charles: {status}"));
        }
        let marker = config.to_string_lossy().into_owned();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            let output = Command::new("ps")
                .args(["-axo", "pid=,command="])
                .output()
                .map_err(|error| error.to_string())?;
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let Some((pid, command)) = line.trim().split_once(char::is_whitespace) else {
                    continue;
                };
                if command.contains(executable.to_string_lossy().as_ref())
                    && command.contains(&marker)
                {
                    return Ok(ProcessRecord {
                        pid: pid
                            .parse()
                            .map_err(|error| format!("invalid process id: {error}"))?,
                        executable,
                        marker,
                    });
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err("managed Charles process was not observed".into())
    }

    fn stop_charles(&self, process: &ProcessRecord) -> Result<bool, String> {
        let output = Command::new("ps")
            .args(["-p", &process.pid.to_string(), "-o", "command="])
            .output()
            .map_err(|error| format!("unable to inspect Charles process: {error}"))?;
        if !output.status.success() {
            return Ok(false);
        }
        let command = String::from_utf8_lossy(&output.stdout);
        if !command.contains(process.executable.to_string_lossy().as_ref())
            || !command.contains(&process.marker)
        {
            return Ok(false);
        }
        let status = Command::new("kill")
            .arg(process.pid.to_string())
            .status()
            .map_err(|error| format!("unable to stop Charles: {error}"))?;
        if !status.success() {
            return Ok(false);
        }

        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            let output = Command::new("ps")
                .args(["-p", &process.pid.to_string(), "-o", "command="])
                .output()
                .map_err(|error| format!("unable to confirm Charles stopped: {error}"))?;
            if !output.status.success() {
                return Ok(true);
            }
            let command = String::from_utf8_lossy(&output.stdout);
            if command.trim().is_empty()
                || !command.contains(process.executable.to_string_lossy().as_ref())
                || !command.contains(&process.marker)
            {
                return Ok(true);
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err("managed Charles process did not exit within 10 seconds".into())
    }

    fn devices(&self) -> Result<Vec<String>, String> {
        let output = Command::new("adb")
            .arg("devices")
            .output()
            .map_err(|error| format!("unable to run adb devices: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once('\t'))
            .filter(|(_, state)| *state == "device")
            .map(|(id, _)| id.to_owned())
            .collect())
    }

    fn ensure_reverse(&self, device: &str, host_port: u16) -> Result<ReverseSnapshot, String> {
        let listed = Self::adb(device, &["reverse", "--list"])?;
        let device_port = host_port;
        let expected = format!("tcp:{device_port} tcp:{host_port}");
        if listed.lines().any(|line| line.contains(&expected)) {
            return Ok(ReverseSnapshot {
                device_id: device.into(),
                device_port,
                host_port,
                owned: false,
            });
        }
        Self::adb(
            device,
            &[
                "reverse",
                &format!("tcp:{device_port}"),
                &format!("tcp:{host_port}"),
            ],
        )?;
        Ok(ReverseSnapshot {
            device_id: device.into(),
            device_port,
            host_port,
            owned: true,
        })
    }

    fn configure_proxy(&self, device: &str, port: u16) -> Result<ProxySnapshot, String> {
        let previous = Self::adb(
            device,
            &["shell", "settings", "get", "global", "http_proxy"],
        )?;
        let configured = format!("127.0.0.1:{port}");
        Self::adb(
            device,
            &[
                "shell",
                "settings",
                "put",
                "global",
                "http_proxy",
                &configured,
            ],
        )?;
        Ok(ProxySnapshot {
            device_id: device.into(),
            previous_value: previous,
            configured_value: configured,
        })
    }

    fn restore_proxy(&self, snapshot: &ProxySnapshot) -> Result<bool, String> {
        let current = Self::adb(
            &snapshot.device_id,
            &["shell", "settings", "get", "global", "http_proxy"],
        )?;
        let Some(previous) = snapshot.restore_value(&current) else {
            return Ok(false);
        };
        Self::adb(
            &snapshot.device_id,
            &["shell", "settings", "put", "global", "http_proxy", previous],
        )?;
        Ok(true)
    }

    fn remove_reverse(&self, snapshot: &ReverseSnapshot) -> Result<bool, String> {
        if !snapshot.owned {
            return Ok(false);
        }
        let listed = Self::adb(&snapshot.device_id, &["reverse", "--list"])?;
        let expected = format!("tcp:{} tcp:{}", snapshot.device_port, snapshot.host_port);
        if !listed.lines().any(|line| line.contains(&expected)) {
            return Ok(false);
        }
        Self::adb(
            &snapshot.device_id,
            &[
                "reverse",
                "--remove",
                &format!("tcp:{}", snapshot.device_port),
            ],
        )?;
        Ok(true)
    }

    fn verify_url(&self, device: &str, url: &str) -> Result<bool, String> {
        let mut expected =
            url::Url::parse(url).map_err(|error| format!("invalid verification URL: {error}"))?;
        expected.set_fragment(Some(&format!("charles-local-mcp-{}", uuid::Uuid::new_v4())));
        Self::adb(
            device,
            &[
                "shell",
                "am",
                "start",
                "-W",
                "-a",
                "android.intent.action.VIEW",
                "-d",
                expected.as_str(),
                "com.android.chrome",
            ],
        )?;

        let forwarded = Self::adb(
            device,
            &["forward", "tcp:0", "localabstract:chrome_devtools_remote"],
        )?;
        let port = forwarded
            .trim()
            .parse::<u16>()
            .map_err(|error| format!("adb returned an invalid Chrome DevTools port: {error}"))?;

        let started = Instant::now();
        let mut consecutive_matches = 0u8;
        let mut verified = false;
        while started.elapsed() < Duration::from_secs(15) {
            if let Ok(targets) = chrome_targets(port) {
                if chrome_target_loaded(&targets, &expected) {
                    consecutive_matches += 1;
                    if consecutive_matches >= 2 {
                        verified = true;
                        break;
                    }
                } else {
                    consecutive_matches = 0;
                }
            }
            thread::sleep(Duration::from_millis(500));
        }

        Self::adb(device, &["forward", "--remove", &format!("tcp:{port}")])?;
        Ok(verified)
    }
}

fn chrome_targets(port: u16) -> Result<serde_json::Value, String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|error| format!("unable to connect to Android Chrome DevTools: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("unable to configure Chrome DevTools timeout: {error}"))?;
    stream
        .write_all(b"GET /json/list HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("unable to query Android Chrome DevTools: {error}"))?;
    let mut response = Vec::new();
    if let Err(error) = stream.read_to_end(&mut response) {
        // Android Chrome keeps its DevTools HTTP/1.1 connection alive even
        // when the request asks it to close. A read timeout after receiving
        // the complete Content-Length body is therefore expected.
        if !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            return Err(format!("unable to read Android Chrome DevTools: {error}"));
        }
    }
    let response = String::from_utf8(response)
        .map_err(|error| format!("Chrome DevTools returned invalid UTF-8: {error}"))?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "Chrome DevTools returned an invalid HTTP response".to_owned())?;
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        return Err("Chrome DevTools target listing failed".into());
    }
    serde_json::from_str(body)
        .map_err(|error| format!("Chrome DevTools returned invalid JSON: {error}"))
}

fn chrome_target_loaded(targets: &serde_json::Value, expected: &url::Url) -> bool {
    let Some(targets) = targets.as_array() else {
        return false;
    };
    targets.iter().any(|target| {
        if target["type"].as_str() != Some("page") {
            return false;
        }
        let Some(candidate) = target["url"]
            .as_str()
            .and_then(|candidate| url::Url::parse(candidate).ok())
        else {
            return false;
        };
        if candidate.scheme() != expected.scheme()
            || candidate.host_str() != expected.host_str()
            || candidate.port_or_known_default() != expected.port_or_known_default()
            || candidate.path() != expected.path()
            || candidate.query() != expected.query()
            || candidate.fragment() != expected.fragment()
        {
            return false;
        }
        let title = target["title"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        ![
            "privacy error",
            "webpage not available",
            "your connection is not private",
            "err_",
            "offline",
        ]
        .iter()
        .any(|marker| title.contains(marker))
    })
}

pub fn require_supported(platform: &dyn Platform) -> Result<(), String> {
    if platform.host_os() != "macos" {
        return Err("mutation is supported on macOS only".into());
    }
    match platform.charles_version()? {
        Some(version) if version == REQUIRED_CHARLES_VERSION => Ok(()),
        Some(version) => Err(format!(
            "unsupported Charles version {version}; expected {REQUIRED_CHARLES_VERSION}"
        )),
        None => Err(format!("Charles {REQUIRED_CHARLES_VERSION} was not found")),
    }
}

pub fn patch_config(input: &[u8], profile: &Profile, proxy_port: u16) -> Result<Vec<u8>, String> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(input.len() + 2048)));
    let mut buffer = Vec::new();
    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut skip_depth = 0usize;
    let mut entry_depth = None;
    let mut map_remote = false;
    let mut replaced_port = false;
    let mut replaced_ssl = false;
    let mut replaced_map = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("invalid Charles XML: {error}"))?;
        if skip_depth > 0 {
            match &event {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => {
                    skip_depth -= 1;
                    if skip_depth == 0 {
                        path.pop();
                    }
                }
                Event::Eof => return Err("unexpected EOF in Charles XML".into()),
                _ => {}
            }
            buffer.clear();
            continue;
        }
        match &event {
            Event::Start(start) => {
                let name = start.name().as_ref().to_vec();
                path.push(name.clone());
                if name == b"entry" {
                    entry_depth = Some(path.len());
                    map_remote = false;
                }
                if path_ends(&path, &[b"configuration", b"proxyConfiguration", b"port"]) {
                    write_raw(&mut writer, format!("<port>{proxy_port}</port>").as_bytes())?;
                    skip_depth = 1;
                    replaced_port = true;
                } else if path_ends(
                    &path,
                    &[b"configuration", b"proxyConfiguration", b"sslLocations"],
                ) {
                    write_raw(&mut writer, ssl_xml(profile).as_bytes())?;
                    skip_depth = 1;
                    replaced_ssl = true;
                } else if name == b"map" && map_remote {
                    write_raw(&mut writer, map_xml(profile).as_bytes())?;
                    skip_depth = 1;
                    replaced_map = true;
                } else {
                    writer
                        .write_event(event.into_owned())
                        .map_err(|error| error.to_string())?;
                }
            }
            Event::Empty(empty) => {
                let name = empty.name().as_ref().to_vec();
                let mut candidate = path.clone();
                candidate.push(name.clone());
                if path_ends(
                    &candidate,
                    &[b"configuration", b"proxyConfiguration", b"sslLocations"],
                ) {
                    write_raw(&mut writer, ssl_xml(profile).as_bytes())?;
                    replaced_ssl = true;
                } else if name == b"map" && map_remote {
                    write_raw(&mut writer, map_xml(profile).as_bytes())?;
                    replaced_map = true;
                } else {
                    writer
                        .write_event(event.into_owned())
                        .map_err(|error| error.to_string())?;
                }
            }
            Event::Text(text) => {
                if path_ends(&path, &[b"entry", b"string"])
                    && String::from_utf8_lossy(text.as_ref()).trim() == "Map Remote"
                {
                    map_remote = true;
                }
                writer
                    .write_event(event.into_owned())
                    .map_err(|error| error.to_string())?;
            }
            Event::End(_) => {
                writer
                    .write_event(event.into_owned())
                    .map_err(|error| error.to_string())?;
                if entry_depth == Some(path.len()) {
                    entry_depth = None;
                    map_remote = false;
                }
                path.pop();
            }
            Event::Eof => break,
            _ => writer
                .write_event(event.into_owned())
                .map_err(|error| error.to_string())?,
        }
        buffer.clear();
    }
    if !(replaced_port && replaced_ssl && replaced_map) {
        return Err(format!("Charles config is missing required sections (port={replaced_port}, ssl={replaced_ssl}, map={replaced_map})"));
    }
    Ok(writer.into_inner().into_inner())
}

fn ssl_xml(profile: &Profile) -> String {
    let mut hosts = profile.ssl_hosts.clone();
    if !hosts.contains(&profile.source_host) {
        hosts.push(profile.source_host.clone());
    }
    let locations = hosts.iter().map(|host| format!("<locationMatch><location><host>{}</host><port>443</port></location><enabled>true</enabled></locationMatch>", xml_escape(host))).collect::<String>();
    format!("<sslLocations><locationPatterns>{locations}</locationPatterns></sslLocations>")
}

fn map_xml(profile: &Profile) -> String {
    let Some(destination_url) = profile.destination_url.as_deref() else {
        return "<map><toolEnabled>false</toolEnabled><mappings/></map>".into();
    };
    let destination = url::Url::parse(destination_url).expect("validated profile URL");
    let source_path = profile
        .source_path
        .as_ref()
        .map(|path| format!("<path>{}</path>", xml_escape(path)))
        .unwrap_or_default();
    let path = if destination.path() == "/" {
        String::new()
    } else {
        format!("<path>{}</path>", xml_escape(destination.path()))
    };
    format!("<map><toolEnabled>true</toolEnabled><mappings><mapMapping><sourceLocation><protocol>https</protocol><host>{}</host>{}</sourceLocation><destLocation><protocol>{}</protocol><host>{}</host><port>{}</port>{}</destLocation><preserveHostHeader>false</preserveHostHeader><enabled>true</enabled></mapMapping></mappings></map>", xml_escape(&profile.source_host), source_path, destination.scheme(), xml_escape(destination.host_str().unwrap()), destination.port_or_known_default().unwrap(), path)
}

fn path_ends(path: &[Vec<u8>], suffix: &[&[u8]]) -> bool {
    path.len() >= suffix.len()
        && path[path.len() - suffix.len()..]
            .iter()
            .map(Vec::as_slice)
            .eq(suffix.iter().copied())
}

fn write_raw(writer: &mut Writer<Cursor<Vec<u8>>>, bytes: &[u8]) -> Result<(), String> {
    writer
        .get_mut()
        .write_all(bytes)
        .map_err(|error| error.to_string())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_config_contains_exact_profile_and_leaves_source_to_caller() {
        let source = include_bytes!("../fixtures/charles.config");
        let profile = Profile {
            source_host: "app.example.com".into(),
            source_path: Some("/source*".into()),
            destination_url: Some("http://127.0.0.1:8080/api".into()),
            ssl_hosts: vec!["api.example.com".into()],
            verification_url: None,
        };
        let output = String::from_utf8(patch_config(source, &profile, 8890).unwrap()).unwrap();
        assert!(output.contains("<port>8890</port>"));
        assert!(output.contains("<host>app.example.com</host>"));
        assert!(output.contains("<path>/source*</path>"));
        assert!(output.contains("<host>127.0.0.1</host>"));
        assert!(output.contains("<path>/api</path>"));
        assert_eq!(source, include_bytes!("../fixtures/charles.config"));
    }

    #[test]
    fn proxy_only_profile_disables_map_remote() {
        let source = include_bytes!("../fixtures/charles.config");
        let profile = Profile {
            source_host: "app.example.com".into(),
            source_path: None,
            destination_url: None,
            ssl_hosts: vec![],
            verification_url: None,
        };
        let output = String::from_utf8(patch_config(source, &profile, 8890).unwrap()).unwrap();
        assert!(output.contains("<toolEnabled>false</toolEnabled><mappings/>"));
        assert!(!output.contains("<mapMapping>"));
    }

    #[test]
    fn chrome_verification_requires_exact_url_and_rejects_error_pages() {
        let expected =
            url::Url::parse("https://app.example.com/health?probe=1#charles-local-mcp-probe")
                .unwrap();
        let success = serde_json::json!([{
            "type": "page",
            "url": "https://app.example.com/health?probe=1#charles-local-mcp-probe",
            "title": "Health check"
        }]);
        assert!(chrome_target_loaded(&success, &expected));

        let stale_target = serde_json::json!([{
            "type": "page",
            "url": "https://app.example.com/health?probe=1",
            "title": "Health check"
        }]);
        assert!(!chrome_target_loaded(&stale_target, &expected));

        let wrong_path = serde_json::json!([{
            "type": "page",
            "url": "https://app.example.com/other?probe=1",
            "title": "Health check"
        }]);
        assert!(!chrome_target_loaded(&wrong_path, &expected));

        let certificate_error = serde_json::json!([{
            "type": "page",
            "url": "https://app.example.com/health?probe=1#charles-local-mcp-probe",
            "title": "Privacy error"
        }]);
        assert!(!chrome_target_loaded(&certificate_error, &expected));
    }
}
