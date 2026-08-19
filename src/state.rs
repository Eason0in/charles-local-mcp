use std::{
    fs,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    model::{DevicePlatform, ProcessRecord, ProxySnapshot, ReverseSnapshot},
    profile::Profile,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSession {
    pub schema_version: u32,
    pub session_id: Uuid,
    pub profile_name: String,
    pub profile: Profile,
    pub platform: DevicePlatform,
    pub device: Option<String>,
    pub managed_config: PathBuf,
    pub proxy_port: u16,
    pub process: Option<ProcessRecord>,
    pub proxy: Option<ProxySnapshot>,
    pub reverse: Option<ReverseSnapshot>,
    pub pending_resume_token: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    Setup,
    Cleanup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRecord {
    pub schema_version: u32,
    pub token: String,
    pub kind: PlanKind,
    pub created_at: i64,
    pub expires_at: i64,
    pub state_hash: String,
    pub profile_name: Option<String>,
    pub profile: Option<Profile>,
    pub platform: Option<DevicePlatform>,
    pub device: Option<String>,
    pub used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeRecord {
    pub schema_version: u32,
    pub token: String,
    pub session_id: Uuid,
    pub expires_at: i64,
    pub used: bool,
}

#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
}

pub struct StateLock {
    file: fs::File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

impl StateStore {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        for path in [
            &root,
            &root.join("plans"),
            &root.join("resumes"),
            &root.join("managed"),
        ] {
            fs::create_dir_all(path)
                .map_err(|error| format!("unable to create {}: {error}", path.display()))?;
            restrict_directory(path)?;
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn lock(&self) -> Result<StateLock, String> {
        let path = self.root.join("state.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("unable to open state lock: {error}"))?;
        restrict_file(&path)?;
        file.lock_exclusive()
            .map_err(|error| format!("unable to lock state: {error}"))?;
        Ok(StateLock { file })
    }

    pub fn load_active(&self) -> Result<Option<ActiveSession>, String> {
        let path = self.root.join("active.json");
        if !path.exists() {
            return Ok(None);
        }
        read_json(&path).map(Some)
    }

    pub fn save_active(&self, value: &ActiveSession) -> Result<(), String> {
        write_json(&self.root.join("active.json"), value)
    }

    pub fn clear_active(&self) -> Result<(), String> {
        remove_if_exists(&self.root.join("active.json"))
    }

    pub fn state_hash(&self) -> Result<String, String> {
        let bytes = match fs::read(self.root.join("active.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => b"none".to_vec(),
            Err(error) => return Err(format!("unable to hash active state: {error}")),
        };
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn create_plan(&self, mut plan: PlanRecord) -> Result<PlanRecord, String> {
        plan.token = Uuid::new_v4().to_string();
        write_json(&self.plan_path(&plan.token)?, &plan)?;
        Ok(plan)
    }

    pub fn consume_plan(
        &self,
        token: &str,
        kind: PlanKind,
        now: i64,
    ) -> Result<PlanRecord, String> {
        let path = self.plan_path(token)?;
        let mut plan: PlanRecord = read_json(&path).map_err(|_| "unknown plan token".to_owned())?;
        if plan.kind != kind {
            return Err("plan token is for a different operation".into());
        }
        if plan.used {
            return Err("plan token has already been used".into());
        }
        if now > plan.expires_at {
            return Err("plan token has expired".into());
        }
        if plan.state_hash != self.state_hash()? {
            return Err("plan token is stale because state changed".into());
        }
        plan.used = true;
        write_json(&path, &plan)?;
        Ok(plan)
    }

    pub fn create_resume(&self, session_id: Uuid, now: i64) -> Result<ResumeRecord, String> {
        let record = ResumeRecord {
            schema_version: 1,
            token: Uuid::new_v4().to_string(),
            session_id,
            expires_at: now + 15 * 60,
            used: false,
        };
        write_json(&self.resume_path(&record.token)?, &record)?;
        Ok(record)
    }

    pub fn consume_resume(&self, token: &str, now: i64) -> Result<ResumeRecord, String> {
        let path = self.resume_path(token)?;
        let mut record: ResumeRecord =
            read_json(&path).map_err(|_| "unknown resume token".to_owned())?;
        if record.used {
            return Err("resume token has already been used".into());
        }
        if now > record.expires_at {
            return Err("resume token has expired".into());
        }
        record.used = true;
        write_json(&path, &record)?;
        Ok(record)
    }

    fn plan_path(&self, token: &str) -> Result<PathBuf, String> {
        token_path(&self.root.join("plans"), token)
    }

    fn resume_path(&self, token: &str) -> Result<PathBuf, String> {
        token_path(&self.root.join("resumes"), token)
    }
}

fn token_path(root: &Path, token: &str) -> Result<PathBuf, String> {
    let parsed = Uuid::parse_str(token).map_err(|_| "invalid token".to_owned())?;
    if parsed.to_string() != token.to_ascii_lowercase() {
        return Err("invalid token".into());
    }
    Ok(root.join(format!("{token}.json")))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("unable to read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid state {}: {error}", path.display()))
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("unable to write {}: {error}", temporary.display()))?;
    restrict_file(&temporary)?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("unable to replace {}: {error}", path.display()))?;
    restrict_file(path)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    write_private(path, &bytes)
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("unable to remove {}: {error}", path.display())),
    }
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), String> {
    Ok(())
}
