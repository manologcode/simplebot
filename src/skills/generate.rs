use super::validate_name;
use crate::process::{backup_file, timestamp, write_atomic};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
struct NewFrontmatter {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility: Option<String>,
    metadata: BTreeMap<String, String>,
}

pub fn render_skill_md(
    name: &str,
    description: &str,
    instructions: &str,
    compatibility: Option<&str>,
    extra_metadata: &BTreeMap<String, String>,
) -> Result<String> {
    validate_name(name)?;
    if description.trim().is_empty() || description.len() > 1024 {
        bail!("description debe tener entre 1 y 1024 caracteres");
    }
    if instructions.trim().is_empty() {
        bail!("las instrucciones de la skill no pueden estar vacias");
    }
    let mut metadata = extra_metadata.clone();
    metadata.insert("simplebot/generated".to_string(), "true".to_string());
    metadata.insert("simplebot/created".to_string(), timestamp());
    let frontmatter = NewFrontmatter {
        name: name.to_string(),
        description: description.trim().to_string(),
        license: None,
        compatibility: compatibility.map(str::to_string),
        metadata,
    };
    let yaml = serde_yaml::to_string(&frontmatter)?;
    let yaml = yaml.trim_end();
    Ok(format!("---\n{yaml}\n---\n\n{}\n", instructions.trim()))
}

#[allow(clippy::too_many_arguments)]
pub fn save_skill(
    root: &Path,
    name: &str,
    description: &str,
    instructions: &str,
    compatibility: Option<&str>,
    extra_metadata: &BTreeMap<String, String>,
    overwrite: bool,
    backup_dir: &Path,
) -> Result<PathBuf> {
    let content = render_skill_md(
        name,
        description,
        instructions,
        compatibility,
        extra_metadata,
    )?;
    let dir = root.join(name);
    let path = dir.join("SKILL.md");
    if path.exists() {
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            bail!("no se sobrescribe un SKILL.md que es enlace simbolico");
        }
        if !overwrite {
            bail!(
                "la skill '{}' ya existe en {}; usa overwrite=true para reemplazarla",
                name,
                path.display()
            );
        }
        backup_file(&path, backup_dir)?;
    }
    fs::create_dir_all(&dir)?;
    write_atomic(&path, content.as_bytes(), 0o644)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_valid_frontmatter() {
        let text = render_skill_md(
            "configurar-wifi",
            "Configura WiFi",
            "## Pasos\n1. Detecta la placa.",
            Some("Raspberry Pi OS, Orange Pi OS"),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(text.starts_with("---\n"));
        assert!(text.contains("name: configurar-wifi"));
        assert!(text.contains("simplebot/generated"));
        assert!(text.contains("## Pasos"));
    }

    #[test]
    fn saves_and_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("backups");
        let path = save_skill(
            dir.path(),
            "test-skill",
            "Una skill",
            "Haz algo",
            None,
            &BTreeMap::new(),
            false,
            &backup,
        )
        .unwrap();
        assert!(path.exists());
        assert!(save_skill(
            dir.path(),
            "test-skill",
            "Otra",
            "Haz otra cosa",
            None,
            &BTreeMap::new(),
            false,
            &backup
        )
        .is_err());
    }

    #[test]
    fn rejects_traversal_name() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save_skill(
            dir.path(),
            "../evil",
            "x",
            "y",
            None,
            &BTreeMap::new(),
            false,
            &dir.path().join("backups")
        )
        .is_err());
    }
}
