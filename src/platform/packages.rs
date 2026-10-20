use crate::process::{run_shell, shell_quote};
use anyhow::{bail, Result};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub message: String,
    pub verified: bool,
}

pub fn install_packages(packages: &[String]) -> Result<InstallReport> {
    if packages.is_empty() {
        bail!("no se indico ningun paquete");
    }
    for package in packages {
        if !valid_package_name(package) {
            bail!("nombre de paquete invalido: {package}");
        }
    }
    let quoted: Vec<String> = packages.iter().map(|p| shell_quote(p)).collect();
    let list = quoted.join(" ");

    let simulate = run_shell(
        &format!("apt-get -s install {list}"),
        Duration::from_secs(60),
        12_000,
        None,
    )?;
    let mut output = String::from("Simulacion:\n");
    output.push_str(simulate.combined().trim());
    output.push_str("\n\n");

    if !simulate.success() {
        let update = run_shell("apt-get update", Duration::from_secs(180), 8_000, None)?;
        if !update.success() {
            bail!(
                "apt-get update fallo antes de instalar {}:\n{}",
                packages.join(", "),
                update.combined()
            );
        }
        output.push_str("apt-get update ejecutado.\n\n");
    }

    let install = run_shell(
        &format!("apt-get install -y --no-install-recommends {list}"),
        Duration::from_secs(600),
        16_000,
        None,
    )?;
    output.push_str(install.combined().trim());
    if !install.success() {
        bail!(
            "la instalacion de {} fallo:\n{}",
            packages.join(", "),
            install.combined()
        );
    }

    let mut verified = Vec::new();
    let mut all_verified = true;
    for package in packages {
        let out = run_shell(
            &format!("dpkg -s {}", shell_quote(package)),
            Duration::from_secs(20),
            4096,
            None,
        )?;
        if !out.success() {
            all_verified = false;
        }
        verified.push(format!(
            "{}: {}",
            package,
            if out.success() {
                "instalado"
            } else {
                "NO verificado"
            }
        ));
    }
    output.push_str("\n\nVerificacion:\n");
    output.push_str(&verified.join("\n"));
    Ok(InstallReport {
        message: output,
        verified: all_verified,
    })
}

fn valid_package_name(package: &str) -> bool {
    !package.is_empty()
        && package.len() <= 128
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+.-_:".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_package_names() {
        assert!(valid_package_name("htop"));
        assert!(valid_package_name("python3-pip"));
        assert!(valid_package_name("libfoo:arm64"));
        assert!(!valid_package_name("foo; rm -rf /"));
        assert!(!valid_package_name(""));
    }
}
