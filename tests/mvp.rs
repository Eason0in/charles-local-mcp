use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use charles_local_mcp::{
    model::{DevicePlatform, ProcessRecord, ProxySnapshot, ResponseStatus, ReverseSnapshot},
    platform::{patch_config, Platform},
    profile::Profile,
    Service, SetupPlanRequest,
};
use serde_json::Value;

#[derive(Debug)]
struct FakePlatform {
    version: String,
    source: PathBuf,
    calls: Mutex<Vec<String>>,
    proxy_current: Mutex<String>,
    verification_ok: bool,
    verification_error: Option<String>,
}

impl FakePlatform {
    fn new(source: PathBuf) -> Self {
        Self {
            version: "4.6.8".into(),
            source,
            calls: Mutex::new(Vec::new()),
            proxy_current: Mutex::new(":0".into()),
            verification_ok: true,
            verification_error: None,
        }
    }

    fn record(&self, call: &str) {
        self.calls.lock().unwrap().push(call.into());
    }
}

impl Platform for FakePlatform {
    fn host_os(&self) -> String {
        "macos".into()
    }
    fn charles_version(&self) -> Result<Option<String>, String> {
        Ok(Some(self.version.clone()))
    }
    fn source_config(&self) -> Result<PathBuf, String> {
        Ok(self.source.clone())
    }
    fn available_port(&self) -> Result<u16, String> {
        Ok(8890)
    }
    fn write_managed_config(
        &self,
        source: &Path,
        target: &Path,
        profile: &Profile,
        port: u16,
    ) -> Result<(), String> {
        self.record("write_config");
        let original = fs::read(source).map_err(|error| error.to_string())?;
        let managed = patch_config(&original, profile, port)?;
        charles_local_mcp::state::write_private(target, &managed)
    }
    fn start_charles(&self, config: &Path) -> Result<ProcessRecord, String> {
        self.record("start_charles");
        Ok(ProcessRecord {
            pid: 42,
            executable: "/Applications/Charles.app/Contents/MacOS/Charles".into(),
            marker: config.display().to_string(),
        })
    }
    fn stop_charles(&self, _process: &ProcessRecord) -> Result<bool, String> {
        self.record("stop_charles");
        Ok(true)
    }
    fn devices(&self) -> Result<Vec<String>, String> {
        Ok(vec!["device-1".into()])
    }
    fn ensure_reverse(&self, device: &str, host_port: u16) -> Result<ReverseSnapshot, String> {
        self.record("ensure_reverse");
        Ok(ReverseSnapshot {
            device_id: device.into(),
            device_port: host_port,
            host_port,
            owned: true,
        })
    }
    fn configure_proxy(&self, device: &str, port: u16) -> Result<ProxySnapshot, String> {
        self.record("configure_proxy");
        let mut current = self.proxy_current.lock().unwrap();
        let previous = current.clone();
        let configured = format!("127.0.0.1:{port}");
        *current = configured.clone();
        Ok(ProxySnapshot {
            device_id: device.into(),
            previous_value: previous,
            configured_value: configured,
        })
    }
    fn restore_proxy(&self, snapshot: &ProxySnapshot) -> Result<bool, String> {
        self.record("restore_proxy");
        let mut current = self.proxy_current.lock().unwrap();
        let Some(previous) = snapshot.restore_value(&current) else {
            return Ok(false);
        };
        *current = previous.into();
        Ok(true)
    }
    fn remove_reverse(&self, _snapshot: &ReverseSnapshot) -> Result<bool, String> {
        self.record("remove_reverse");
        Ok(true)
    }
    fn verify_url(&self, _device: &str, _url: &str) -> Result<bool, String> {
        self.record("verify_url");
        if let Some(error) = &self.verification_error {
            return Err(error.clone());
        }
        Ok(self.verification_ok)
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.config");
    fs::write(&source, include_bytes!("../fixtures/charles.config")).unwrap();
    let profiles = directory.path().join("profiles.toml");
    fs::write(
        &profiles,
        r#"schemaVersion = 1

[profiles.demo]
sourceHost = "app.example.com"
destinationUrl = "http://127.0.0.1:8080"
sslHosts = ["api.example.com"]
verificationUrl = "https://app.example.com/health"

[profiles.other]
sourceHost = "other.example.com"
destinationUrl = "http://localhost:3000"
"#,
    )
    .unwrap();
    (directory, source, profiles)
}

fn service(
    directory: &tempfile::TempDir,
    profiles: PathBuf,
    platform: Arc<FakePlatform>,
) -> Service {
    Service::with_platform(directory.path().join("state"), Some(profiles), platform).unwrap()
}

fn extract_token(response: &charles_local_mcp::Response) -> String {
    response.data.as_ref().unwrap()["token"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn wrong_charles_version_is_rejected_before_mutation() {
    let (directory, source, profiles) = fixture();
    let mut fake = FakePlatform::new(source);
    fake.version = "4.6.7".into();
    let fake = Arc::new(fake);
    let service = service(&directory, profiles, fake.clone());
    let plan = service.setup_plan(SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Host,
        device: None,
    });
    let result = service.setup_apply(&extract_token(&plan));
    assert_eq!(result.status, ResponseStatus::Error);
    assert_eq!(result.error.unwrap().code, "unsupported_host");
    assert!(fake.calls.lock().unwrap().is_empty());
}

#[test]
fn setup_is_idempotent_and_only_one_different_session_is_allowed() {
    let (directory, source, profiles) = fixture();
    let fake = Arc::new(FakePlatform::new(source));
    let service = service(&directory, profiles, fake.clone());
    let request = SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Host,
        device: None,
    };
    let first = service.setup_plan(request.clone());
    let first = service.setup_apply(&extract_token(&first));
    assert_eq!(first.status, ResponseStatus::Ready);
    let session = first.data.as_ref().unwrap()["sessionId"].clone();

    let second = service.setup_plan(request);
    let second = service.setup_apply(&extract_token(&second));
    assert_eq!(second.data.as_ref().unwrap()["sessionId"], session);
    assert_eq!(second.data.as_ref().unwrap()["idempotent"], true);
    assert_eq!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| *call == "start_charles")
            .count(),
        1
    );

    let conflict = service.setup_plan(SetupPlanRequest {
        profile: "other".into(),
        platform: DevicePlatform::Host,
        device: None,
    });
    assert_eq!(conflict.status, ResponseStatus::Error);
    assert_eq!(conflict.error.unwrap().code, "active_session_conflict");
}

#[test]
fn token_is_single_use_and_stale_state_is_rejected() {
    let (directory, source, profiles) = fixture();
    let fake = Arc::new(FakePlatform::new(source));
    let service = service(&directory, profiles, fake.clone());
    let plan = service.setup_plan(SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Host,
        device: None,
    });
    let token = extract_token(&plan);
    assert_eq!(service.setup_apply(&token).status, ResponseStatus::Ready);
    let reused = service.setup_apply(&token);
    assert_eq!(reused.error.unwrap().code, "token_used");

    let cleanup = service.cleanup_plan();
    let cleanup_token = extract_token(&cleanup);
    let active_path = directory.path().join("state/active.json");
    let mut active: Value = serde_json::from_slice(&fs::read(&active_path).unwrap()).unwrap();
    active["schemaVersion"] = 2.into();
    fs::write(&active_path, serde_json::to_vec_pretty(&active).unwrap()).unwrap();
    let stale = service.cleanup_apply(&cleanup_token);
    assert_eq!(stale.error.unwrap().code, "stale_state");
}

#[test]
fn expired_token_is_rejected_without_mutation() {
    let (directory, source, profiles) = fixture();
    let fake = Arc::new(FakePlatform::new(source));
    let service = service(&directory, profiles, fake.clone());
    let plan = service.setup_plan(SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Host,
        device: None,
    });
    let token = extract_token(&plan);
    let plan_path = directory
        .path()
        .join("state/plans")
        .join(format!("{token}.json"));
    let mut record: Value = serde_json::from_slice(&fs::read(&plan_path).unwrap()).unwrap();
    record["expiresAt"] = 0.into();
    fs::write(&plan_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let result = service.setup_apply(&token);
    assert_eq!(result.error.unwrap().code, "token_expired");
    assert!(fake.calls.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn managed_config_is_private_and_source_is_byte_for_byte_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let (directory, source, profiles) = fixture();
    let original = fs::read(&source).unwrap();
    let fake = Arc::new(FakePlatform::new(source.clone()));
    let service = service(&directory, profiles, fake);
    let plan = service.setup_plan(SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Host,
        device: None,
    });
    let applied = service.setup_apply(&extract_token(&plan));
    assert_eq!(applied.status, ResponseStatus::Ready);
    let managed = PathBuf::from(
        applied.data.unwrap()["active"]["managedConfig"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(fs::read(source).unwrap(), original);
    assert_eq!(
        fs::metadata(managed).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn android_cleanup_uses_compare_and_swap_and_owned_order() {
    let (directory, source, profiles) = fixture();
    let fake = Arc::new(FakePlatform::new(source));
    let service = service(&directory, profiles, fake.clone());
    let plan = service.setup_plan(SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Android,
        device: Some("device-1".into()),
    });
    let applied = service.setup_apply(&extract_token(&plan));
    assert_eq!(applied.status, ResponseStatus::Ready);
    *fake.proxy_current.lock().unwrap() = "external-proxy:9000".into();
    let cleanup = service.cleanup_plan();
    let cleaned = service.cleanup_apply(&extract_token(&cleanup));
    assert_eq!(cleaned.status, ResponseStatus::Ready);
    assert_eq!(*fake.proxy_current.lock().unwrap(), "external-proxy:9000");
    let calls = fake.calls.lock().unwrap();
    let restore = calls
        .iter()
        .position(|call| call == "restore_proxy")
        .unwrap();
    let reverse = calls
        .iter()
        .position(|call| call == "remove_reverse")
        .unwrap();
    let stop = calls
        .iter()
        .position(|call| call == "stop_charles")
        .unwrap();
    assert!(restore < reverse && reverse < stop);
    assert_eq!(service.status().data.unwrap()["active"], Value::Null);
}

#[test]
fn ca_checkpoint_has_single_use_resume_token() {
    let (directory, source, profiles) = fixture();
    let mut fake = FakePlatform::new(source);
    fake.verification_ok = false;
    let service = service(&directory, profiles, Arc::new(fake));
    let plan = service.setup_plan(SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Android,
        device: Some("device-1".into()),
    });
    let applied = service.setup_apply(&extract_token(&plan));
    assert_eq!(applied.status, ResponseStatus::NeedsUserAction);
    let resume = applied.checkpoint.unwrap().resume_token;
    assert_eq!(service.setup_resume(&resume).status, ResponseStatus::Ready);
    assert_eq!(
        service.setup_resume(&resume).error.unwrap().code,
        "token_used"
    );
}

#[test]
fn setup_error_rolls_back_proxy_reverse_process_and_state() {
    let (directory, source, profiles) = fixture();
    let mut fake = FakePlatform::new(source);
    fake.verification_error = Some("Chrome DevTools connection failed".into());
    let fake = Arc::new(fake);
    let service = service(&directory, profiles, fake.clone());
    let plan = service.setup_plan(SetupPlanRequest {
        profile: "demo".into(),
        platform: DevicePlatform::Android,
        device: Some("device-1".into()),
    });

    let applied = service.setup_apply(&extract_token(&plan));

    assert_eq!(applied.status, ResponseStatus::Error);
    assert!(applied
        .error
        .unwrap()
        .message
        .contains("setup changes were rolled back"));
    assert_eq!(*fake.proxy_current.lock().unwrap(), ":0");
    assert_eq!(service.status().data.unwrap()["active"], Value::Null);
    assert!(!directory
        .path()
        .join("state/managed")
        .read_dir()
        .unwrap()
        .any(|entry| entry.is_ok()));
    let calls = fake.calls.lock().unwrap();
    let verify = calls.iter().position(|call| call == "verify_url").unwrap();
    let restore = calls
        .iter()
        .position(|call| call == "restore_proxy")
        .unwrap();
    let reverse = calls
        .iter()
        .position(|call| call == "remove_reverse")
        .unwrap();
    let stop = calls
        .iter()
        .position(|call| call == "stop_charles")
        .unwrap();
    assert!(verify < restore && restore < reverse && reverse < stop);
}
