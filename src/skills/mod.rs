pub mod generate;

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: PathBuf,
    pub compatibility: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    pub skills: Vec<Skill>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    license: Option<String>,
    compatibility: Option<String>,
    metadata: Option<BTreeMap<String, serde_yaml::Value>>,
}

impl SkillRegistry {
    pub fn discover(paths: &[PathBuf], max_bytes: usize) -> Self {
        let mut registry = SkillRegistry::default();
        let mut seen = HashSet::new();
        for root in paths {
            let root = crate::config::expand_path(root);
            if !root.is_dir() {
                continue;
            }
            let canonical_root = match fs::canonicalize(&root) {
                Ok(path) => path,
                Err(err) => {
                    registry.warnings.push(format!("{}: {err}", root.display()));
                    continue;
                }
            };
            let entries = match fs::read_dir(&root) {
                Ok(entries) => entries,
                Err(err) => {
                    registry.warnings.push(format!("{}: {err}", root.display()));
                    continue;
                }
            };
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let skill_file = dir.join("SKILL.md");
                if !skill_file.is_file() {
                    continue;
                }
                match load_skill(&canonical_root, &skill_file, max_bytes) {
                    Ok(skill) => {
                        if let Ok(meta) = fs::metadata(&skill_file) {
                            if meta.permissions().mode() & 0o002 != 0 {
                                registry.warnings.push(format!(
                                    "la skill '{}' es escribible por otros usuarios: {}",
                                    skill.name,
                                    skill_file.display()
                                ));
                            }
                        }
                        if seen.contains(&skill.name) {
                            registry.warnings.push(format!(
                                "skill duplicada '{}' en {}; se conserva la primera",
                                skill.name,
                                skill_file.display()
                            ));
                        } else {
                            seen.insert(skill.name.clone());
                            registry.skills.push(skill);
                        }
                    }
                    Err(err) => registry
                        .warnings
                        .push(format!("{}: {err}", skill_file.display())),
                }
            }
        }
        registry.skills.sort_by(|a, b| a.name.cmp(&b.name));
        registry
    }

    pub fn find(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    pub fn prompt(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut text = String::from("<available_skills>\n");
        for skill in &self.skills {
            text.push_str(&format!(
                "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>\n",
                xml_escape(&skill.name),
                xml_escape(&skill.description)
            ));
        }
        text.push_str("</available_skills>");
        text
    }
}

fn load_skill(root: &Path, path: &Path, max_bytes: usize) -> Result<Skill> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        bail!("SKILL.md no puede ser un enlace simbolico");
    }
    if meta.len() as usize > max_bytes {
        bail!("SKILL.md supera el limite de {} bytes", max_bytes);
    }
    let canonical = fs::canonicalize(path)?;
    if !canonical.starts_with(root) {
        bail!("la skill sale del directorio permitido");
    }
    let text = fs::read_to_string(&canonical)
        .with_context(|| format!("no se pudo leer {}", canonical.display()))?;
    let (frontmatter, body) = parse_frontmatter(&text)?;
    validate_name(&frontmatter.name)?;
    let dir_name = canonical
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if dir_name != frontmatter.name {
        bail!(
            "el nombre '{}' no coincide con el directorio '{}'",
            frontmatter.name,
            dir_name
        );
    }
    if frontmatter.description.trim().is_empty() || frontmatter.description.len() > 1024 {
        bail!("description debe tener entre 1 y 1024 caracteres");
    }
    let mut metadata = BTreeMap::new();
    if let Some(map) = frontmatter.metadata {
        for (key, value) in map {
            let value = match value {
                serde_yaml::Value::String(s) => s,
                serde_yaml::Value::Bool(b) => b.to_string(),
                serde_yaml::Value::Number(n) => n.to_string(),
                other => serde_yaml::to_string(&other)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            };
            metadata.insert(key, value);
        }
    }
    let _ = frontmatter.license;
    Ok(Skill {
        name: frontmatter.name,
        description: frontmatter.description,
        body: body.trim_start_matches('\n').to_string(),
        path: canonical,
        compatibility: frontmatter.compatibility,
        metadata,
    })
}

fn parse_frontmatter(text: &str) -> Result<(Frontmatter, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !text.starts_with("---") {
        bail!("falta el frontmatter YAML al inicio del SKILL.md");
    }
    let rest = &text[3..];
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        bail!("no se encontro el cierre del frontmatter YAML");
    };
    let yaml = &rest[..end];
    let body_start = end + 4;
    let body = rest.get(body_start..).unwrap_or("");
    let frontmatter: Frontmatter =
        serde_yaml::from_str(yaml).context("frontmatter YAML invalido")?;
    Ok((frontmatter, body.to_string()))
}

pub fn validate_name(name: &str) -> Result<()> {
    let re = Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("regex valida");
    if name.is_empty() || name.len() > 64 || !re.is_match(name) {
        bail!(
            "nombre de skill invalido '{}': usa minusculas, numeros y guiones simples (1-64)",
            name
        );
    }
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_skill(root: &Path, name: &str, description: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("SKILL.md");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "---\nname: {name}\ndescription: {description}\nmetadata:\n  simplebot/boards: raspberry-pi\n---\n\n## Pasos\nHaz algo."
        )
        .unwrap();
        path
    }

    #[test]
    fn validates_names() {
        assert!(validate_name("configurar-wifi").is_ok());
        assert!(validate_name("Configurar Wifi").is_err());
        assert!(validate_name("-wifi").is_err());
        assert!(validate_name("wifi--lan").is_err());
    }

    #[test]
    fn discovers_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "configurar-wifi", "Configura WiFi");
        let registry = SkillRegistry::discover(&[dir.path().to_path_buf()], 32_768);
        assert_eq!(registry.skills.len(), 1);
        assert_eq!(registry.skills[0].name, "configurar-wifi");
        assert!(registry.skills[0].body.contains("Haz algo"));
        assert!(registry.prompt().contains("configurar-wifi"));
    }

    #[test]
    fn rejects_name_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let wrong = dir.path().join("otro");
        fs::create_dir_all(&wrong).unwrap();
        fs::write(
            wrong.join("SKILL.md"),
            "---\nname: distinto\ndescription: x\n---\nbody",
        )
        .unwrap();
        let registry = SkillRegistry::discover(&[dir.path().to_path_buf()], 32_768);
        assert!(registry.skills.is_empty());
        assert!(!registry.warnings.is_empty());
    }
}
