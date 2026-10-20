use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub provider: ProviderConfig,
    pub agent: AgentConfig,
    pub permissions: PermissionsConfig,
    pub ui: UiConfig,
    pub skills: SkillsConfig,
    pub platform: PlatformConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_key_env: String,
    pub api_key_file: Option<PathBuf>,
    pub user_agent: String,
    pub timeout_secs: u64,
    pub max_context_tokens: usize,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: "https://opencode.ai/inference/go/openai/v1".to_string(),
            model: "deepseek-v4.1-flash".to_string(),
            api_key: None,
            api_key_env: "OPENCODE_API_KEY".to_string(),
            api_key_file: None,
            user_agent: concat!("simplebot/", env!("CARGO_PKG_VERSION")).to_string(),
            timeout_secs: 120,
            max_context_tokens: 32_000,
            temperature: 0.1,
            max_tokens: 4_096,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_steps: usize,
    pub max_minutes: u64,
    pub max_tool_output_bytes: usize,
    pub language: String,
    pub max_identical_failures: usize,
    pub session_log: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_steps: 40,
            max_minutes: 20,
            max_tool_output_bytes: 12_000,
            language: "es".to_string(),
            max_identical_failures: 3,
            session_log: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    #[default]
    Risky,
    ConfirmAll,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallPolicy {
    #[default]
    Ask,
    Never,
    Allow,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PermissionsConfig {
    pub mode: PermissionMode,
    pub install: InstallPolicy,
    pub allow_dangerous: bool,
    pub audit_log: PathBuf,
    pub backup_dir: PathBuf,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Risky,
            install: InstallPolicy::Ask,
            allow_dangerous: false,
            audit_log: PathBuf::from("/var/log/simplebot/audit.jsonl"),
            backup_dir: PathBuf::from("/var/backups/simplebot"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct UiConfig {
    pub color: bool,
    pub show_commands: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            color: true,
            show_commands: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationMode {
    #[default]
    Confirm,
    Off,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SkillsConfig {
    pub paths: Vec<PathBuf>,
    pub generation: GenerationMode,
    pub max_loaded: usize,
    pub max_bytes: usize,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            paths: default_skill_paths(),
            generation: GenerationMode::Confirm,
            max_loaded: 5,
            max_bytes: 32_768,
        }
    }
}

fn default_skill_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/etc/simplebot/skills"),
        PathBuf::from("~/.config/simplebot/skills"),
        PathBuf::from("~/.config/opencode/skills"),
        PathBuf::from("~/.agents/skills"),
        PathBuf::from("~/.claude/skills"),
        PathBuf::from("./.simplebot/skills"),
        PathBuf::from("./.opencode/skills"),
        PathBuf::from("./.agents/skills"),
        PathBuf::from("./.claude/skills"),
    ]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PlatformConfig {
    pub profile: String,
    pub network_backend: String,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            profile: "auto".to_string(),
            network_backend: "auto".to_string(),
        }
    }
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<(Self, Option<PathBuf>)> {
        let path = explicit
            .map(Path::to_path_buf)
            .or_else(|| env::var_os("SIMPLEBOT_CONFIG").map(PathBuf::from))
            .or_else(|| {
                [
                    PathBuf::from("/etc/simplebot/config.toml"),
                    home_dir().join(".config/simplebot/config.toml"),
                    PathBuf::from("simplebot.toml"),
                ]
                .into_iter()
                .find(|p| p.is_file())
            });

        let Some(path) = path else {
            return Ok((Self::default(), None));
        };

        let text = fs::read_to_string(&path)
            .with_context(|| format!("no se pudo leer la configuracion {}", path.display()))?;
        let mut config: Config = toml::from_str(&text)
            .with_context(|| format!("configuracion TOML invalida en {}", path.display()))?;
        config.normalize();
        config.validate()?;
        Ok((config, Some(path)))
    }

    fn normalize(&mut self) {
        self.provider.base_url = self.provider.base_url.trim_end_matches('/').to_string();
        if let Some(file) = &self.provider.api_key_file {
            self.provider.api_key_file = Some(expand_path(file));
        }
        self.permissions.audit_log = expand_path(&self.permissions.audit_log);
        self.permissions.backup_dir = expand_path(&self.permissions.backup_dir);
        self.skills.paths = self.skills.paths.iter().map(|p| expand_path(p)).collect();
    }

    fn validate(&self) -> Result<()> {
        if self.provider.base_url.is_empty() {
            bail!("provider.base_url no puede estar vacio");
        }
        if self.provider.model.is_empty() {
            bail!("provider.model no puede estar vacio");
        }
        if self.provider.timeout_secs == 0 {
            bail!("provider.timeout_secs debe ser mayor que cero");
        }
        if self.provider.max_context_tokens < 1024 {
            bail!("provider.max_context_tokens debe ser al menos 1024");
        }
        if self.agent.max_steps == 0 {
            bail!("agent.max_steps debe ser mayor que cero");
        }
        if self.agent.max_minutes == 0 {
            bail!("agent.max_minutes debe ser mayor que cero");
        }
        if self.agent.max_tool_output_bytes < 512 {
            bail!("agent.max_tool_output_bytes debe ser al menos 512");
        }
        if self.skills.max_loaded == 0 {
            bail!("skills.max_loaded debe ser mayor que cero");
        }
        Ok(())
    }

    pub fn api_key(&self) -> Result<String> {
        if let Some(key) = self.provider.api_key.as_deref() {
            if !key.trim().is_empty() {
                return Ok(key.trim().to_string());
            }
        }
        if let Ok(key) = env::var(&self.provider.api_key_env) {
            if !key.trim().is_empty() {
                return Ok(key.trim().to_string());
            }
        }
        if let Some(file) = &self.provider.api_key_file {
            match fs::read_to_string(file) {
                Ok(key) if !key.trim().is_empty() => return Ok(key.trim().to_string()),
                Ok(_) => {}
                Err(err) => bail!(
                    "falta la API key: define {} o crea {} ({err})",
                    self.provider.api_key_env,
                    file.display()
                ),
            }
        }
        bail!(
            "falta la API key: define {} o configura provider.api_key_file",
            self.provider.api_key_env
        )
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|_| "<config invalida>".to_string())
    }
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"))
}

pub fn expand_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        return home_dir();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn parses_partial_config() {
        let cfg: Config = toml::from_str(
            r#"
            [provider]
            model = "glm-5.3-flash"

            [permissions]
            mode = "confirm_all"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.provider.model, "glm-5.3-flash");
        assert_eq!(cfg.permissions.mode, PermissionMode::ConfirmAll);
        assert_eq!(cfg.agent.max_steps, 40);
    }

    #[test]
    fn expands_home() {
        let p = expand_path(Path::new("~/skills"));
        assert!(p.ends_with("skills"));
        assert!(!p.to_string_lossy().starts_with('~'));
    }
}
