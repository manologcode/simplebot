use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::time::Duration;
use ureq::Agent;

const REPO: &str = "manologcode/simplebot";
const USER_AGENT: &str = concat!("simplebot/", env!("CARGO_PKG_VERSION"));
const MAX_CHECKSUM_BYTES: u64 = 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
}

pub fn run(check_only: bool, force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let release = fetch_latest_release()?;
    let latest = release.tag_name.trim().to_string();

    println!("version actual: {current}");
    println!("ultima version: {latest}");

    if !force && !version_is_newer(&latest, current) {
        if check_only {
            println!("no hay actualizaciones disponibles");
        } else {
            println!("ya estas en la ultima version");
        }
        return Ok(());
    }

    if check_only {
        println!("hay una nueva version disponible: {latest}");
        return Ok(());
    }

    let asset = asset_name()?;
    let tag = release.tag_name.trim();
    validate_tag(tag)?;
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let agent = http_agent(Duration::from_secs(300));

    println!("descargando {asset}...");
    let sums_url = format!("{base}/SHA256SUMS");
    let sums = http_get(&agent, &sums_url, MAX_CHECKSUM_BYTES)
        .with_context(|| format!("no se pudo descargar {sums_url}"))?;
    let sums = String::from_utf8(sums).context("SHA256SUMS no es texto UTF-8")?;
    let expected = expected_sha256(&sums, asset)
        .with_context(|| format!("SHA256SUMS no contiene la entrada para {asset}"))?;

    let binary_url = format!("{base}/{asset}");
    let bytes = http_get(&agent, &binary_url, MAX_BINARY_BYTES)
        .with_context(|| format!("no se pudo descargar {binary_url}"))?;
    if bytes.len() < 4 || &bytes[..4] != b"\x7fELF" {
        bail!("el fichero descargado no es un binario ELF valido");
    }

    let actual = sha256_hex(&bytes);
    if actual != expected {
        bail!("checksum invalido para {asset}: esperado {expected}, obtenido {actual}");
    }

    let path = replace_current_binary(&bytes)?;
    println!("actualizado a {latest} en {}", path.display());
    println!("reinicia simplebot para usar la nueva version");
    Ok(())
}

fn http_agent(timeout: Duration) -> Agent {
    ureq::AgentBuilder::new().timeout(timeout).build()
}

fn fetch_latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let agent = http_agent(Duration::from_secs(30));
    let body = http_get(&agent, &url, MAX_CHECKSUM_BYTES)
        .with_context(|| format!("no se pudo consultar {url}"))?;
    serde_json::from_slice(&body).context("respuesta de GitHub invalida")
}

fn http_get(agent: &Agent, url: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let response = agent
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call();
    match response {
        Ok(resp) => {
            let mut reader = resp.into_reader().take(max_bytes + 1);
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .context("no se pudo leer la respuesta")?;
            if bytes.len() as u64 > max_bytes {
                bail!("la respuesta supera el limite de {max_bytes} bytes");
            }
            Ok(bytes)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!(
                "HTTP {}: {}",
                code,
                crate::process::truncate_middle(&body, 500)
            )
        }
        Err(err) => bail!("error de red: {err}"),
    }
}

fn asset_name() -> Result<&'static str> {
    match env::consts::ARCH {
        "x86_64" => Ok("simplebot-linux-amd64"),
        "aarch64" => Ok("simplebot-linux-arm64"),
        other => bail!("arquitectura no soportada para actualizar: {other}"),
    }
}

fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty()
        || !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    {
        bail!("tag de release invalido: {tag:?}");
    }
    Ok(())
}

fn expected_sha256(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn replace_current_binary(bytes: &[u8]) -> Result<PathBuf> {
    let exe = env::current_exe().context("no se pudo localizar el ejecutable actual")?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    let dir = exe
        .parent()
        .context("el ejecutable actual no tiene directorio padre")?;
    let tmp = dir.join(format!(".simplebot-update-{}.tmp", std::process::id()));

    let result: Result<()> = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o755)
            .open(&tmp)
            .with_context(|| {
                format!(
                    "no se pudo escribir en {}. Si esta instalado en /usr/local/bin, ejecuta: sudo simplebot update",
                    dir.display()
                )
            })?;
        file.write_all(bytes)
            .context("no se pudo escribir el binario")?;
        file.sync_all()
            .context("no se pudo sincronizar el binario")?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))
            .context("no se pudieron ajustar los permisos")?;
        fs::rename(&tmp, &exe)
            .with_context(|| format!("no se pudo reemplazar {}", exe.display()))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    Ok(exe)
}

fn version_is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(mut latest), Some(mut current)) => {
            let len = latest.len().max(current.len());
            latest.resize(len, 0);
            current.resize(len, 0);
            latest > current
        }
        _ => latest.trim_start_matches('v') != current.trim_start_matches('v'),
    }
}

fn parse_version(value: &str) -> Option<Vec<u64>> {
    let core = value
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let parts: Vec<u64> = core
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    (!parts.is_empty()).then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(version_is_newer("v0.2.0", "0.1.1"));
        assert!(version_is_newer("0.1.10", "0.1.9"));
        assert!(version_is_newer("1.0", "0.9.9"));
        assert!(!version_is_newer("v0.1.1", "0.1.1"));
        assert!(!version_is_newer("0.1.0", "0.1"));
        assert!(!version_is_newer("v0.0.9", "0.1.0"));
    }

    #[test]
    fn parses_release_versions() {
        assert_eq!(parse_version("v0.1.1"), Some(vec![0, 1, 1]));
        assert_eq!(parse_version("1.2.3-beta.1"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("no-es-version"), None);
    }

    #[test]
    fn finds_asset_checksum() {
        let sums = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  simplebot-linux-amd64\n\
                    BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB *simplebot-linux-arm64\n";
        assert_eq!(
            expected_sha256(sums, "simplebot-linux-arm64").as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert!(expected_sha256(sums, "simplebot-linux-riscv64").is_none());
    }

    #[test]
    fn rejects_invalid_tags() {
        assert!(validate_tag("v1.2.3").is_ok());
        assert!(validate_tag("../evil").is_err());
        assert!(validate_tag("v1 2").is_err());
        assert!(validate_tag("").is_err());
    }

    #[test]
    fn hashes_bytes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
