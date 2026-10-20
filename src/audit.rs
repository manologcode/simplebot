use crate::safety::redact::redact;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AuditLogger {
    path: PathBuf,
    secrets: Vec<String>,
}

impl AuditLogger {
    pub fn new(path: PathBuf, secrets: Vec<String>) -> Self {
        Self { path, secrets }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn log(&self, event: Value) {
        let text = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
        let text = redact(&text, &self.secrets);
        if let Some(parent) = self.path.parent() {
            if fs::create_dir_all(parent).is_err() {
                log::warn!(
                    "no se pudo crear el directorio de auditoria {}",
                    parent.display()
                );
                return;
            }
        }
        if fs::metadata(&self.path)
            .map(|meta| meta.len() > 10 * 1024 * 1024)
            .unwrap_or(false)
        {
            let rotated = self.path.with_extension("jsonl.1");
            let _ = fs::rename(&self.path, rotated);
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path);
        match file {
            Ok(mut file) => {
                let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
                let _ = writeln!(file, "{text}");
            }
            Err(err) => log::warn!("no se pudo escribir la auditoria: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_redacted_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(path.clone(), vec!["s3cr3t".to_string()]);
        logger.log(json!({"command": "echo s3cr3t", "ok": true}));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("[REDACTED]"));
        assert!(!text.contains("s3cr3t"));
    }
}
