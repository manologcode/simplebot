use crate::process::{command_exists, run_readonly};
use serde::Serialize;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BoardKind {
    RaspberryPi,
    OrangePi4Pro,
    GenericArm,
    Other,
}

impl BoardKind {
    pub fn label(self) -> &'static str {
        match self {
            BoardKind::RaspberryPi => "Raspberry Pi",
            BoardKind::OrangePi4Pro => "Orange Pi 4 Pro",
            BoardKind::GenericArm => "ARM64 generico",
            BoardKind::Other => "otro",
        }
    }

    pub fn matches_profile(self, profile: &str) -> bool {
        match profile {
            "auto" => true,
            "raspberry-pi" => self == BoardKind::RaspberryPi,
            "orange-pi-4-pro" => self == BoardKind::OrangePi4Pro,
            "generic-debian" => self != BoardKind::RaspberryPi && self != BoardKind::OrangePi4Pro,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardInfo {
    pub kind: BoardKind,
    pub model: String,
    pub distro: String,
    pub arch: String,
    pub kernel: String,
    pub package_manager: String,
}

impl BoardInfo {
    pub fn detect(profile_override: &str) -> Self {
        let model = read_device_model();
        let mut kind = classify_model(&model, std::env::consts::ARCH);
        if profile_override != "auto" {
            kind = match profile_override {
                "raspberry-pi" => BoardKind::RaspberryPi,
                "orange-pi-4-pro" => BoardKind::OrangePi4Pro,
                "generic-debian" => BoardKind::GenericArm,
                _ => kind,
            };
        }
        let distro = read_os_release();
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
            .to_string();
        let package_manager = if command_exists("apt-get") {
            "apt"
        } else if command_exists("dnf") {
            "dnf"
        } else if command_exists("pacman") {
            "pacman"
        } else {
            "desconocido"
        };
        Self {
            kind,
            model: if model.is_empty() {
                "desconocido".to_string()
            } else {
                model
            },
            distro,
            arch: std::env::consts::ARCH.to_string(),
            kernel,
            package_manager: package_manager.to_string(),
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "{} | {} | {} | {} | kernel {} | paquetes: {}",
            self.kind.label(),
            self.model,
            self.distro,
            self.arch,
            self.kernel,
            self.package_manager
        )
    }
}

fn classify_model(model: &str, arch: &str) -> BoardKind {
    let lower = model.to_lowercase();
    let orange = lower.contains("orange pi") || lower.contains("orangepi");
    if lower.contains("raspberry pi") {
        BoardKind::RaspberryPi
    } else if (orange && lower.contains("4 pro"))
        || lower.contains("a733")
        || lower.contains("sun60i")
    {
        BoardKind::OrangePi4Pro
    } else if orange || arch == "aarch64" {
        BoardKind::GenericArm
    } else {
        BoardKind::Other
    }
}

fn read_device_model() -> String {
    for path in [
        "/proc/device-tree/model",
        "/sys/firmware/devicetree/base/model",
    ] {
        if let Ok(bytes) = fs::read(path) {
            let text = String::from_utf8_lossy(&bytes).replace('\0', "");
            let text = text.trim();
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    let dmi = fs::read_to_string("/sys/class/dmi/id/product_name").unwrap_or_default();
    dmi.trim().to_string()
}

fn read_os_release() -> String {
    let Ok(text) = fs::read_to_string("/etc/os-release") else {
        return "desconocido".to_string();
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            return value.trim_matches('"').to_string();
        }
    }
    "desconocido".to_string()
}

pub fn default_route_interface() -> Option<String> {
    let out = run_readonly("ip route show default", 4096).ok()?;
    parse_default_route_interface(&out.stdout)
}

fn parse_default_route_interface(text: &str) -> Option<String> {
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let mut last_dev = None;
        while let Some(word) = words.next() {
            if word == "dev" {
                last_dev = words.next();
            }
        }
        if let Some(dev) = last_dev {
            return Some(dev.to_string());
        }
    }
    None
}

pub fn wireless_interfaces() -> Vec<String> {
    let mut result = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("wireless").exists() {
            result.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    result.sort();
    result
}

pub fn is_wireless(interface: &str) -> bool {
    std::path::Path::new("/sys/class/net")
        .join(interface)
        .join("wireless")
        .exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_route() {
        let text = "default via 192.168.1.1 dev wlan0 proto dhcp metric 600\n";
        assert_eq!(
            parse_default_route_interface(text).as_deref(),
            Some("wlan0")
        );
    }

    #[test]
    fn classifies_models() {
        assert_eq!(
            classify_model("Raspberry Pi 4 Model B", "aarch64"),
            BoardKind::RaspberryPi
        );
        assert_eq!(
            classify_model("OrangePi 4 Pro", "aarch64"),
            BoardKind::OrangePi4Pro
        );
        assert_eq!(
            classify_model("Orange Pi 4 Pro", "aarch64"),
            BoardKind::OrangePi4Pro
        );
        assert_eq!(
            classify_model("OrangePi Zero3", "aarch64"),
            BoardKind::GenericArm
        );
        assert!(BoardKind::RaspberryPi.matches_profile("raspberry-pi"));
        assert!(BoardKind::OrangePi4Pro.matches_profile("orange-pi-4-pro"));
        assert!(!BoardKind::OrangePi4Pro.matches_profile("raspberry-pi"));
    }
}
