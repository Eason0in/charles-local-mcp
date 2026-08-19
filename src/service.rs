use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use uuid::Uuid;

use crate::{
    model::{Checkpoint, DevicePlatform, Response, SetupPlanRequest},
    platform::{require_supported, Platform, RealPlatform},
    profile::ProfilesFile,
    state::{ActiveSession, PlanKind, PlanRecord, StateStore},
};

#[derive(Clone)]
pub struct Service {
    store: StateStore,
    profiles_file: Option<PathBuf>,
    platform: Arc<dyn Platform>,
}

impl Service {
    pub fn new(state_dir: PathBuf, profiles_file: Option<PathBuf>) -> Result<Self, String> {
        Self::with_platform(state_dir, profiles_file, Arc::new(RealPlatform))
    }

    pub fn with_platform(
        state_dir: PathBuf,
        profiles_file: Option<PathBuf>,
        platform: Arc<dyn Platform>,
    ) -> Result<Self, String> {
        Ok(Self {
            store: StateStore::new(state_dir)?,
            profiles_file,
            platform,
        })
    }

    pub fn doctor(&self) -> Response {
        let version = self.platform.charles_version();
        match version {
            Ok(version) => Response::ready(
                "doctor",
                json!({
                    "hostOs": self.platform.host_os(),
                    "hostSupported": self.platform.host_os() == "macos",
                    "charlesVersion": version.clone(),
                    "requiredCharlesVersion": crate::model::REQUIRED_CHARLES_VERSION,
                    "charlesSupported": version.as_deref() == Some(crate::model::REQUIRED_CHARLES_VERSION),
                    "stateDirectory": self.store.root(),
                    "profilesFile": self.profiles_file,
                }),
            ),
            Err(error) => Response::error("doctor", "inspection_failed", error),
        }
    }

    pub fn profiles_list(&self) -> Response {
        match self.load_profiles() {
            Ok(file) => Response::ready(
                "profiles.list",
                json!({"schemaVersion": file.schema_version, "profiles": file.profiles.keys().collect::<Vec<_>>() }),
            ),
            Err(error) => Response::error("profiles.list", "invalid_profiles", error),
        }
    }

    pub fn profiles_validate(&self) -> Response {
        match self.load_profiles() {
            Ok(file) => Response::ready(
                "profiles.validate",
                json!({"valid": true, "profileCount": file.profiles.len()}),
            ),
            Err(error) => Response::error("profiles.validate", "invalid_profiles", error),
        }
    }

    pub fn devices_list(&self) -> Response {
        match self.platform.devices() {
            Ok(devices) => Response::ready("devices.list", json!({"devices": devices})),
            Err(error) => Response::error("devices.list", "device_inspection_failed", error),
        }
    }

    pub fn setup_plan(&self, request: SetupPlanRequest) -> Response {
        self.wrap("setup.plan", || {
            let _lock = self.store.lock()?;
            let profiles = self.load_profiles()?;
            let profile = profiles
                .profiles
                .get(&request.profile)
                .cloned()
                .ok_or_else(|| format!("unknown profile {:?}", request.profile))?;
            if request.platform == DevicePlatform::Android && request.device.is_none() {
                let devices = self.platform.devices()?;
                if devices.len() != 1 {
                    return Err(format!(
                        "Android setup requires --device unless exactly one device is connected (found {})",
                        devices.len()
                    ));
                }
            }
            if let Some(active) = self.store.load_active()? {
                let requested_device = request
                    .device
                    .clone()
                    .or_else(|| (request.platform == DevicePlatform::Android).then(|| self.platform.devices().ok()).flatten().and_then(|devices| devices.into_iter().next()));
                if active.profile_name != request.profile
                    || active.platform != request.platform
                    || active.device != requested_device
                {
                    return Err(format!(
                        "active session {} must be cleaned up before a different setup",
                        active.session_id
                    ));
                }
            }
            let now = now();
            let device = match (request.platform, request.device) {
                (DevicePlatform::Android, None) => self.platform.devices()?.into_iter().next(),
                (_, device) => device,
            };
            let plan = self.store.create_plan(PlanRecord {
                schema_version: 1,
                token: String::new(),
                kind: PlanKind::Setup,
                created_at: now,
                expires_at: now + 15 * 60,
                state_hash: self.store.state_hash()?,
                profile_name: Some(request.profile),
                profile: Some(profile),
                platform: Some(request.platform),
                device,
                used: false,
            })?;
            Ok(Response::ready(
                "setup.plan",
                json!({
                    "token": plan.token,
                    "expiresAt": plan.expires_at,
                    "singleUse": true,
                    "actions": setup_actions(plan.platform.unwrap()),
                }),
            ))
        })
    }

    pub fn setup_apply(&self, token: &str) -> Response {
        self.wrap("setup.apply", || {
            let _lock = self.store.lock()?;
            let plan = self.store.consume_plan(token, PlanKind::Setup, now())?;
            require_supported(self.platform.as_ref())?;
            if let Some(active) = self.store.load_active()? {
                return Ok(Response::ready(
                    "setup.apply",
                    json!({"sessionId": active.session_id, "idempotent": true, "active": active}),
                ));
            }
            let profile = plan.profile.ok_or_else(|| "setup plan has no profile".to_owned())?;
            let platform = plan.platform.ok_or_else(|| "setup plan has no platform".to_owned())?;
            let session_id = Uuid::new_v4();
            let source = self.platform.source_config()?;
            if !source.is_file() {
                return Err(format!("Charles source configuration does not exist: {}", source.display()));
            }
            let port = self.platform.available_port()?;
            let managed = self.store.root().join("managed").join(format!("{session_id}.config"));
            self.platform.write_managed_config(&source, &managed, &profile, port)?;
            let process = match self.platform.start_charles(&managed) {
                Ok(process) => process,
                Err(error) => {
                    let _ = fs::remove_file(&managed);
                    return Err(error);
                }
            };
            let mut active = ActiveSession {
                schema_version: 1,
                session_id,
                profile_name: plan.profile_name.expect("setup profile name"),
                profile,
                platform,
                device: plan.device,
                managed_config: managed,
                proxy_port: port,
                process: Some(process),
                proxy: None,
                reverse: None,
                pending_resume_token: None,
            };
            if let Err(error) = self.store.save_active(&active) {
                let rollback = self.rollback_setup(&mut active);
                return Err(with_rollback_error(error, rollback));
            }

            let result = (|| -> Result<Response, String> {
                if platform == DevicePlatform::Android {
                    let device = active
                        .device
                        .clone()
                        .ok_or_else(|| "Android device is missing".to_owned())?;
                    let reverse = self.platform.ensure_reverse(&device, port)?;
                    active.reverse = Some(reverse);
                    self.store.save_active(&active)?;
                    let proxy = self.platform.configure_proxy(&device, port)?;
                    active.proxy = Some(proxy);
                    self.store.save_active(&active)?;
                    if let Some(url) = active.profile.verification_url.as_deref() {
                        if !self.platform.verify_url(&device, url)? {
                            let resume = self.store.create_resume(session_id, now())?;
                            active.pending_resume_token = Some(resume.token.clone());
                            self.store.save_active(&active)?;
                            return Ok(Response::needs_action(
                                "setup.apply",
                                json!({"sessionId": session_id, "active": active}),
                                Checkpoint {
                                    kind: "verify_android_ca".into(),
                                    instruction: "Confirm Android trusts the Charles CA and the HTTPS verification URL loads, then resume.".into(),
                                    resume_token: resume.token,
                                    expires_at: resume.expires_at,
                                },
                            ));
                        }
                    }
                }

                if platform == DevicePlatform::Ios {
                    let resume = self.store.create_resume(session_id, now())?;
                    active.pending_resume_token = Some(resume.token.clone());
                    self.store.save_active(&active)?;
                    return Ok(Response::needs_action(
                        "setup.apply",
                        json!({"sessionId": session_id, "active": active}),
                        Checkpoint {
                            kind: "configure_ios_proxy".into(),
                            instruction: format!("Configure the iOS Wi-Fi proxy to this Mac on port {port}, then resume."),
                            resume_token: resume.token,
                            expires_at: resume.expires_at,
                        },
                    ));
                }
                Ok(Response::ready(
                    "setup.apply",
                    json!({"sessionId": session_id, "active": active}),
                ))
            })();

            match result {
                Ok(response) => Ok(response),
                Err(error) => {
                    let rollback = self.rollback_setup(&mut active);
                    Err(with_rollback_error(error, rollback))
                }
            }
        })
    }

    pub fn setup_resume(&self, token: &str) -> Response {
        self.wrap("setup.resume", || {
            let _lock = self.store.lock()?;
            let resume = self.store.consume_resume(token, now())?;
            let mut active = self
                .store
                .load_active()?
                .ok_or_else(|| "there is no active session".to_owned())?;
            if active.session_id != resume.session_id
                || active.pending_resume_token.as_deref() != Some(token)
            {
                return Err("resume token does not match the active session".into());
            }
            active.pending_resume_token = None;
            self.store.save_active(&active)?;
            Ok(Response::ready(
                "setup.resume",
                json!({"sessionId": active.session_id, "active": active}),
            ))
        })
    }

    pub fn status(&self) -> Response {
        self.wrap("status", || {
            let _lock = self.store.lock()?;
            let active = self.store.load_active()?;
            Ok(Response::ready("status", json!({"active": active})))
        })
    }

    pub fn cleanup_plan(&self) -> Response {
        self.wrap("cleanup.plan", || {
            let _lock = self.store.lock()?;
            let active = self.store.load_active()?;
            let now = now();
            let plan = self.store.create_plan(PlanRecord {
                schema_version: 1,
                token: String::new(),
                kind: PlanKind::Cleanup,
                created_at: now,
                expires_at: now + 15 * 60,
                state_hash: self.store.state_hash()?,
                profile_name: None,
                profile: None,
                platform: None,
                device: None,
                used: false,
            })?;
            Ok(Response::ready(
                "cleanup.plan",
                json!({"token": plan.token, "expiresAt": plan.expires_at, "singleUse": true, "active": active}),
            ))
        })
    }

    pub fn cleanup_apply(&self, token: &str) -> Response {
        self.wrap("cleanup.apply", || {
            let _lock = self.store.lock()?;
            let _plan = self.store.consume_plan(token, PlanKind::Cleanup, now())?;
            require_supported(self.platform.as_ref())?;
            let Some(mut active) = self.store.load_active()? else {
                return Ok(Response::ready(
                    "cleanup.apply",
                    json!({"cleaned": false, "reason": "no_active_session"}),
                ));
            };
            let mut evidence = Vec::new();
            if let Some(proxy) = active.proxy.clone() {
                let restored = self.platform.restore_proxy(&proxy)?;
                evidence.push(json!({"kind": "androidProxy", "restored": restored}));
                active.proxy = None;
                self.store.save_active(&active)?;
            }
            if let Some(reverse) = active.reverse.clone() {
                let removed = self.platform.remove_reverse(&reverse)?;
                evidence.push(json!({"kind": "adbReverse", "removed": removed}));
                active.reverse = None;
                self.store.save_active(&active)?;
            }
            if let Some(process) = active.process.clone() {
                let stopped = self.platform.stop_charles(&process)?;
                evidence.push(json!({"kind": "charlesProcess", "stopped": stopped}));
                active.process = None;
                self.store.save_active(&active)?;
            }
            if active
                .managed_config
                .starts_with(self.store.root().join("managed"))
            {
                match fs::remove_file(&active.managed_config) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("unable to remove managed config: {error}")),
                }
            }
            self.store.clear_active()?;
            Ok(Response::ready(
                "cleanup.apply",
                json!({"cleaned": true, "sessionId": active.session_id, "evidence": evidence}),
            ))
        })
    }

    fn load_profiles(&self) -> Result<ProfilesFile, String> {
        let path = self
            .profiles_file
            .as_deref()
            .ok_or_else(|| "--profiles-file is required for this operation".to_owned())?;
        ProfilesFile::load(path)
    }

    fn rollback_setup(&self, active: &mut ActiveSession) -> Vec<String> {
        let mut errors = Vec::new();
        if let Some(proxy) = active.proxy.clone() {
            match self.platform.restore_proxy(&proxy) {
                Ok(_) => {
                    active.proxy = None;
                    if let Err(error) = self.store.save_active(active) {
                        errors.push(error);
                    }
                }
                Err(error) => errors.push(format!("unable to restore Android proxy: {error}")),
            }
        }
        if let Some(reverse) = active.reverse.clone() {
            match self.platform.remove_reverse(&reverse) {
                Ok(_) => {
                    active.reverse = None;
                    if let Err(error) = self.store.save_active(active) {
                        errors.push(error);
                    }
                }
                Err(error) => errors.push(format!("unable to remove ADB reverse: {error}")),
            }
        }
        if let Some(process) = active.process.clone() {
            match self.platform.stop_charles(&process) {
                Ok(_) => {
                    active.process = None;
                    if let Err(error) = self.store.save_active(active) {
                        errors.push(error);
                    }
                }
                Err(error) => errors.push(format!("unable to stop managed Charles: {error}")),
            }
        }

        if active.proxy.is_none() && active.reverse.is_none() && active.process.is_none() {
            if active
                .managed_config
                .starts_with(self.store.root().join("managed"))
            {
                match fs::remove_file(&active.managed_config) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => errors.push(format!("unable to remove managed config: {error}")),
                }
            }
            if errors.is_empty() {
                if let Err(error) = self.store.clear_active() {
                    errors.push(error);
                }
            }
        }

        if !errors.is_empty() {
            if let Err(error) = self.store.save_active(active) {
                errors.push(format!("unable to preserve recovery state: {error}"));
            }
        }
        errors
    }

    fn wrap(&self, operation: &str, action: impl FnOnce() -> Result<Response, String>) -> Response {
        match action() {
            Ok(response) => response,
            Err(error) => Response::error(operation, classify_error(&error), error),
        }
    }
}

fn with_rollback_error(error: String, rollback: Vec<String>) -> String {
    if rollback.is_empty() {
        format!("{error}; setup changes were rolled back")
    } else {
        format!(
            "{error}; automatic rollback was incomplete: {}",
            rollback.join("; ")
        )
    }
}

fn setup_actions(platform: DevicePlatform) -> Vec<&'static str> {
    let mut actions = vec![
        "validate Charles 4.6.8",
        "create private managed config",
        "start managed Charles",
    ];
    if platform == DevicePlatform::Android {
        actions.extend([
            "create or reuse ADB reverse",
            "set Android proxy with compare-and-swap snapshot",
        ]);
    }
    if platform == DevicePlatform::Ios {
        actions.push("request manual iOS proxy configuration");
    }
    actions
}

fn classify_error(error: &str) -> &'static str {
    if error.contains("expired") {
        "token_expired"
    } else if error.contains("already been used") {
        "token_used"
    } else if error.contains("stale") {
        "stale_state"
    } else if error.contains("active session") {
        "active_session_conflict"
    } else if error.contains("Charles version")
        || error.contains("macOS only")
        || error.contains("was not found")
    {
        "unsupported_host"
    } else if error.contains("profile")
        || error.contains("destinationUrl")
        || error.contains("sourceHost")
    {
        "invalid_profile"
    } else {
        "operation_failed"
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn default_state_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("CHARLES_LOCAL_MCP_HOME") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/charles-local-mcp")
}
