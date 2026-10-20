use crate::process::{is_root, timestamp};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SessionLogger {
    path: Option<PathBuf>,
}

impl SessionLogger {
    pub fn new(session_id: &str, enabled: bool) -> Self {
        if !enabled {
            return Self { path: None };
        }
        let base = if is_root() {
            PathBuf::from("/var/log/simplebot/sessions")
        } else {
            crate::config::home_dir().join(".local/share/simplebot/sessions")
        };
        if let Err(err) = fs::create_dir_all(&base) {
            log::warn!("no se pudo crear {}: {err}", base.display());
            return Self { path: None };
        }
        Self {
            path: Some(base.join(format!("{session_id}.jsonl"))),
        }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn log(&self, event: Value) {
        let Some(path) = &self.path else {
            return;
        };
        let text = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path);
        if let Ok(mut file) = file {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
            let _ = writeln!(file, "{text}");
        }
    }

    pub fn log_line(&self, kind: &str, content: &str) {
        self.log(serde_json::json!({
            "ts": timestamp(),
            "kind": kind,
            "content": content,
        }));
    }
}
