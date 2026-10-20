use crate::platform::board::{is_wireless, wireless_interfaces};
use crate::process::{
    backup_file, command_exists, is_ssh_session, run_readonly, run_shell, shell_quote, timestamp,
    write_atomic, CommandOutput,
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum NetworkBackend {
    NetworkManager,
    Netplan,
    SystemdNetworkd,
    Unknown,
}

impl NetworkBackend {
    pub fn label(self) -> &'static str {
        match self {
            NetworkBackend::NetworkManager => "NetworkManager",
            NetworkBackend::Netplan => "netplan",
            NetworkBackend::SystemdNetworkd => "systemd-networkd",
            NetworkBackend::Unknown => "desconocido",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MutationReport {
    pub message: String,
    pub verified: bool,
}

pub fn detect_backend(override_value: &str) -> NetworkBackend {
    match override_value {
        "networkmanager" => return NetworkBackend::NetworkManager,
        "netplan" => return NetworkBackend::Netplan,
        "systemd-networkd" => return NetworkBackend::SystemdNetworkd,
        _ => {}
    }
    if command_exists("nmcli") {
        if let Ok(out) = run_readonly("nmcli -t -f RUNNING general", 1024) {
            if out.stdout.to_lowercase().contains("running") {
                return NetworkBackend::NetworkManager;
            }
        }
    }
    if command_exists("netplan") && Path::new("/etc/netplan").is_dir() {
        return NetworkBackend::Netplan;
    }
    if command_exists("networkctl") {
        if let Ok(out) = run_readonly("systemctl is-active systemd-networkd", 1024) {
            if out.stdout.trim() == "active" {
                return NetworkBackend::SystemdNetworkd;
            }
        }
    }
    NetworkBackend::Unknown
}

pub fn status_report(backend: NetworkBackend, max_bytes: usize) -> String {
    let mut parts = Vec::new();
    parts.push(format!("Backend: {}", backend.label()));
    parts.push(format!(
        "Interfaces inalambricas: {}",
        if wireless_interfaces().is_empty() {
            "ninguna".to_string()
        } else {
            wireless_interfaces().join(", ")
        }
    ));
    let commands: Vec<&str> = match backend {
        NetworkBackend::NetworkManager => vec![
            "nmcli general status",
            "nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device status",
            "nmcli -t -f NAME,DEVICE,TYPE connection show --active",
        ],
        NetworkBackend::Netplan => vec!["netplan get", "networkctl status"],
        NetworkBackend::SystemdNetworkd => vec![
            "networkctl status",
            "systemctl status systemd-networkd --no-pager -n 20",
        ],
        NetworkBackend::Unknown => {
            vec!["systemctl status NetworkManager systemd-networkd --no-pager -n 20"]
        }
    };
    for cmd in commands {
        match run_readonly(cmd, max_bytes) {
            Ok(out) => {
                let text = out.combined();
                parts.push(format!("$ {cmd}\n{}", text.trim()));
            }
            Err(err) => parts.push(format!("$ {cmd}\n(error: {err})")),
        }
    }
    for cmd in [
        "ip -brief address",
        "ip route",
        "resolvectl status",
        "cat /etc/resolv.conf",
    ] {
        if let Ok(out) = run_readonly(cmd, max_bytes) {
            let text = out.combined();
            if !text.trim().is_empty() {
                parts.push(format!("$ {cmd}\n{}", text.trim()));
            }
        }
    }
    parts.join("\n\n")
}

pub fn wifi_connect(
    backend: NetworkBackend,
    interface: Option<&str>,
    ssid: &str,
    password: Option<&str>,
    hidden: bool,
    backup_dir: &Path,
) -> Result<MutationReport> {
    if ssid.trim().is_empty() {
        bail!("el SSID no puede estar vacio");
    }
    let iface = interface
        .filter(|i| !i.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            if let NetworkBackend::NetworkManager = backend {
                nm_wifi_interface().ok().flatten()
            } else {
                wireless_interfaces().into_iter().next()
            }
        })
        .context("no se encontro ninguna interfaz inalambrica")?;

    match backend {
        NetworkBackend::NetworkManager => {
            nm_wifi_connect(&iface, ssid, password, hidden, backup_dir)
        }
        NetworkBackend::Netplan => netplan_wifi_connect(&iface, ssid, password, hidden, backup_dir),
        NetworkBackend::SystemdNetworkd => {
            networkd_wifi_connect(&iface, ssid, password, backup_dir)
        }
        NetworkBackend::Unknown => bail!(
            "no se pudo detectar un backend de red compatible; revisa la salida de network_status"
        ),
    }
}

pub fn set_static_ip(
    backend: NetworkBackend,
    interface: &str,
    ip_cidr: &str,
    gateway: &str,
    dns: &[String],
    backup_dir: &Path,
) -> Result<MutationReport> {
    if !ip_cidr.contains('/') {
        bail!("la IP debe incluir prefijo CIDR, por ejemplo 192.168.1.50/24");
    }
    match backend {
        NetworkBackend::NetworkManager => {
            nm_set_static(interface, ip_cidr, gateway, dns, backup_dir)
        }
        NetworkBackend::Netplan => netplan_set_static(interface, ip_cidr, gateway, dns, backup_dir),
        NetworkBackend::SystemdNetworkd => {
            networkd_set_static(interface, ip_cidr, gateway, dns, backup_dir)
        }
        NetworkBackend::Unknown => bail!(
            "no se pudo detectar un backend de red compatible; revisa la salida de network_status"
        ),
    }
}

pub fn ssh_risk_for(interface: &str) -> bool {
    if !is_ssh_session() {
        return false;
    }
    let (iface, _) = current_default_route();
    iface.as_deref() == Some(interface)
}

pub fn verify_with_retry(interface: &str, gateway: Option<&str>) -> (bool, String) {
    let mut last = String::new();
    for attempt in 0..5 {
        let (ok, output) = verify_interface(interface, gateway);
        last = output;
        if ok {
            return (true, last);
        }
        if attempt < 4 {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    (false, last)
}

pub fn verify_interface(interface: &str, gateway: Option<&str>) -> (bool, String) {
    let mut sections = Vec::new();
    let addr = run_readonly(
        &format!("ip -4 -brief address show dev {}", shell_quote(interface)),
        4096,
    )
    .unwrap_or_else(|e| crate::process::CommandOutput {
        status: None,
        stdout: format!("error: {e}"),
        stderr: String::new(),
        timed_out: false,
        duration_ms: 0,
    });
    let route = run_readonly("ip route show default", 4096).unwrap_or_else(|e| {
        crate::process::CommandOutput {
            status: None,
            stdout: format!("error: {e}"),
            stderr: String::new(),
            timed_out: false,
            duration_ms: 0,
        }
    });
    sections.push(format!(
        "$ ip -4 -brief address show dev {interface}\n{}",
        addr.combined().trim()
    ));
    sections.push(format!(
        "$ ip route show default\n{}",
        route.combined().trim()
    ));

    let (default_iface, default_gateway) = parse_default_route(&route.stdout);
    let has_address = addr.stdout.split_whitespace().any(|w| w.contains('.'));
    let has_default = default_iface.as_deref() == Some(interface) && default_gateway.is_some();
    let gateway = gateway
        .map(str::to_string)
        .or(default_gateway)
        .filter(|g| !g.is_empty());

    let mut gateway_ok = false;
    if let Some(gw) = &gateway {
        let ping = verify_cmd(&format!("ping -c 1 -W 3 {}", shell_quote(gw)), 2048);
        gateway_ok = ping.success();
        sections.push(format!("$ ping -c 1 -W 3 {gw}\n{}", ping.combined().trim()));
    }

    let external = verify_cmd("ping -c 1 -W 3 1.1.1.1", 1024);
    let external_ok = external.success();
    sections.push(format!(
        "$ ping -c 1 -W 3 1.1.1.1\n{}",
        external.combined().trim()
    ));

    let dns = verify_cmd("getent hosts opencode.ai", 2048);
    sections.push(format!(
        "$ getent hosts opencode.ai\n{}",
        dns.combined().trim()
    ));

    let success = has_address && (gateway_ok || external_ok) && (has_default || gateway_ok);
    sections.push(format!(
        "Resultado: ipv4={} ruta_default={} ping_gateway={} ping_externo={} dns={}",
        has_address,
        has_default,
        gateway_ok,
        external_ok,
        dns.success()
    ));
    (success, sections.join("\n\n"))
}

fn verify_cmd(command: &str, max_bytes: usize) -> CommandOutput {
    run_shell(command, Duration::from_secs(5), max_bytes, None).unwrap_or_else(|err| {
        CommandOutput {
            status: None,
            stdout: format!("error: {err}"),
            stderr: String::new(),
            timed_out: false,
            duration_ms: 0,
        }
    })
}

fn current_default_route() -> (Option<String>, Option<String>) {
    let out = run_readonly("ip route show default", 4096).unwrap_or_else(|_| {
        crate::process::CommandOutput {
            status: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            duration_ms: 0,
        }
    });
    parse_default_route(&out.stdout)
}

fn parse_default_route(text: &str) -> (Option<String>, Option<String>) {
    for line in text.lines() {
        if !line.trim_start().starts_with("default") {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let mut iface = None;
        let mut gateway = None;
        let mut idx = 0;
        while idx < words.len() {
            match words[idx] {
                "dev" if idx + 1 < words.len() => {
                    iface = Some(words[idx + 1].to_string());
                    idx += 2;
                }
                "via" if idx + 1 < words.len() => {
                    gateway = Some(words[idx + 1].to_string());
                    idx += 2;
                }
                _ => idx += 1,
            }
        }
        if iface.is_some() {
            return (iface, gateway);
        }
    }
    (None, None)
}

fn nm_wifi_interface() -> Result<Option<String>> {
    let out = run_readonly("nmcli -t -f DEVICE,TYPE device status", 4096)?;
    for line in out.stdout.lines() {
        let mut parts = line.split(':');
        let device = parts.next().unwrap_or("");
        let kind = parts.next().unwrap_or("");
        if kind == "wifi" && !device.is_empty() {
            return Ok(Some(device.to_string()));
        }
    }
    Ok(None)
}

fn nm_wifi_connect(
    iface: &str,
    ssid: &str,
    password: Option<&str>,
    hidden: bool,
    backup_dir: &Path,
) -> Result<MutationReport> {
    let sanitized: String = ssid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let connection_id = format!("simplebot-{sanitized}");
    let path = Path::new("/etc/NetworkManager/system-connections")
        .join(format!("{connection_id}.nmconnection"));
    let backup = backup_file(&path, backup_dir)?;

    let mut content = format!(
        "[connection]\nid={}\nuuid={}\ntype=wifi\ninterface-name={}\nautoconnect=true\nautoconnect-priority=100\n\n[wifi]\nmode=infrastructure\nssid={}\nhidden={}\n\n",
        keyfile_escape(&connection_id),
        Uuid::new_v4(),
        keyfile_escape(iface),
        keyfile_escape(ssid),
        if hidden { "true" } else { "false" }
    );
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        content.push_str(&format!(
            "[wifi-security]\nkey-mgmt=wpa-psk\npsk={}\n\n",
            keyfile_escape(password)
        ));
    } else {
        content.push_str("[wifi-security]\nkey-mgmt=none\n\n");
    }
    content.push_str("[ipv4]\nmethod=auto\nmay-fail=false\n\n[ipv6]\nmethod=auto\n");
    write_atomic(&path, content.as_bytes(), 0o600)?;

    let reload = run_shell(
        "nmcli connection reload",
        Duration::from_secs(20),
        4096,
        None,
    )?;
    if !reload.success() {
        restore_nm_profile(&path, backup.as_ref(), &connection_id);
        bail!("nmcli no pudo recargar los perfiles: {}", reload.combined());
    }
    let up = run_shell(
        &format!("nmcli connection up id {}", shell_quote(&connection_id)),
        Duration::from_secs(90),
        8192,
        None,
    )?;
    if !up.success() {
        restore_nm_profile(&path, backup.as_ref(), &connection_id);
        bail!(
            "no se pudo activar la conexion WiFi '{}': {}",
            connection_id,
            up.combined()
        );
    }
    let (verified, verification) = verify_with_retry(iface, None);
    if !verified {
        restore_nm_profile(&path, backup.as_ref(), &connection_id);
        bail!("WiFi configurado pero la verificacion fallo:\n{verification}");
    }
    Ok(MutationReport {
        message: format!(
            "WiFi '{}' conectado en {} mediante NetworkManager.\n\n{}",
            ssid, iface, verification
        ),
        verified,
    })
}

fn restore_nm_profile(path: &Path, backup: Option<&PathBuf>, connection_id: &str) {
    if let Some(saved) = backup {
        let _ = fs::copy(saved, path);
    } else {
        let _ = fs::remove_file(path);
    }
    let _ = run_shell(
        "nmcli connection reload",
        Duration::from_secs(20),
        4096,
        None,
    );
    if backup.is_some() {
        let _ = run_shell(
            &format!("nmcli connection up id {}", shell_quote(connection_id)),
            Duration::from_secs(60),
            4096,
            None,
        );
    }
}

fn nm_set_static(
    iface: &str,
    ip_cidr: &str,
    gateway: &str,
    dns: &[String],
    backup_dir: &Path,
) -> Result<MutationReport> {
    let connection = nm_active_connection(iface)?
        .with_context(|| format!("no hay una conexion NetworkManager activa para {iface}"))?;
    let uuid = connection.0;
    let name = connection.1;
    let file = nm_connection_file(&uuid).ok();

    let stamp = timestamp().replace([':', '-'], "");
    let backup_path = backup_dir.join(format!("{stamp}-rollback-{iface}.sh"));
    let mut script = String::from("#!/bin/bash\nset -u\n");
    if let Some(file) = &file {
        if let Some(saved) = backup_file(file, backup_dir)? {
            script.push_str(&format!(
                "cp {} {} && nmcli connection reload\n",
                shell_quote(&saved.to_string_lossy()),
                shell_quote(&file.to_string_lossy())
            ));
        }
    } else {
        script.push_str(&format!(
            "nmcli connection modify {} ipv4.method auto ipv4.addresses '' ipv4.gateway '' ipv4.dns '' ipv4.ignore-auto-dns no\n",
            shell_quote(&uuid)
        ));
    }
    script.push_str(&format!("nmcli connection up id {}\n", shell_quote(&name)));
    let rollback = write_rollback_script(&backup_path, &script)?;
    let unit = schedule_rollback(&rollback);

    let dns_arg = dns.join(",");
    let mut command = format!(
        "nmcli connection modify {} ipv4.method manual ipv4.addresses {} ipv4.gateway {}",
        shell_quote(&uuid),
        shell_quote(ip_cidr),
        shell_quote(gateway)
    );
    if !dns_arg.is_empty() {
        command.push_str(&format!(
            " ipv4.dns {} ipv4.ignore-auto-dns yes",
            shell_quote(&dns_arg)
        ));
    }
    let modify = run_shell(&command, Duration::from_secs(30), 8192, None)?;
    if !modify.success() {
        cancel_rollback(unit.as_deref());
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        bail!("nmcli no pudo aplicar la IP fija: {}", modify.combined());
    }
    let up = run_shell(
        &format!("nmcli connection up id {}", shell_quote(&name)),
        Duration::from_secs(90),
        8192,
        None,
    )?;
    if !up.success() {
        cancel_rollback(unit.as_deref());
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        bail!(
            "la conexion no subio con la IP fija; se restauro el perfil anterior: {}",
            up.combined()
        );
    }
    let (verified, verification) = verify_with_retry(iface, Some(gateway));
    if !verified {
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        cancel_rollback(unit.as_deref());
        let _ = std::fs::remove_file(&rollback);
        bail!(
            "la IP fija no paso la verificacion; se restauro el perfil anterior:\n{verification}"
        );
    }
    cancel_rollback(unit.as_deref());
    let _ = std::fs::remove_file(&rollback);
    Ok(MutationReport {
        message: format!(
            "IP fija aplicada en {iface}: {ip_cidr}, gateway {gateway}, DNS {}.\n\n{}",
            if dns.is_empty() {
                "sin cambios".to_string()
            } else {
                dns.join(", ")
            },
            verification
        ),
        verified,
    })
}

fn nm_active_connection(iface: &str) -> Result<Option<(String, String)>> {
    let out = run_readonly(
        "nmcli -t -f UUID,NAME,DEVICE connection show --active",
        8192,
    )?;
    for line in out.stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() == 3 && parts[2] == iface {
            return Ok(Some((parts[0].to_string(), parts[1].to_string())));
        }
    }
    let out = run_readonly("nmcli -t -f UUID,NAME,DEVICE connection show", 8192)?;
    for line in out.stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() == 3 && parts[2] == iface {
            return Ok(Some((parts[0].to_string(), parts[1].to_string())));
        }
    }
    Ok(None)
}

fn nm_connection_file(uuid: &str) -> Result<PathBuf> {
    let out = run_readonly(
        &format!("nmcli -t -f FILENAME connection show {}", shell_quote(uuid)),
        4096,
    )?;
    for line in out.stdout.lines() {
        if line.starts_with('/') && Path::new(line).exists() {
            return Ok(PathBuf::from(line));
        }
    }
    if let Some(path) = find_nm_connection_file(uuid) {
        return Ok(path);
    }
    bail!("no se pudo localizar el fichero del perfil {uuid}")
}

fn find_nm_connection_file(uuid: &str) -> Option<PathBuf> {
    let dir = Path::new("/etc/NetworkManager/system-connections");
    let needle = format!("uuid={uuid}");
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("nmconnection") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if text
                .lines()
                .any(|line| line.trim().replace(' ', "") == needle)
            {
                return Some(path);
            }
        }
    }
    None
}

fn netplan_renderer() -> Result<String> {
    let dir = Path::new("/etc/netplan");
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("no se pudo leer {}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("yaml"))
        .collect();
    files.sort();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("renderer:") {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Ok(value.to_string());
                }
            }
        }
    }
    Ok("networkd".to_string())
}

fn netplan_wifi_connect(
    iface: &str,
    ssid: &str,
    password: Option<&str>,
    hidden: bool,
    backup_dir: &Path,
) -> Result<MutationReport> {
    let renderer = netplan_renderer()?;
    if renderer.to_lowercase().contains("networkmanager") {
        bail!("netplan esta usando el renderer NetworkManager; usa platform.network_backend = \"networkmanager\"");
    }
    let file = Path::new("/etc/netplan").join(format!("90-simplebot-{iface}.yaml"));
    let rollback_path = prepare_rollback_file(&file, backup_dir, "netplan apply")?;
    let unit = schedule_rollback(&rollback_path);

    let mut ap = serde_json::Map::new();
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        ap.insert("password".to_string(), json!(password));
    }
    if hidden {
        ap.insert("hidden".to_string(), json!(true));
    }
    let doc = json!({
        "network": {
            "version": 2,
            "renderer": renderer,
            "wifis": {
                iface: {
                    "dhcp4": true,
                    "access-points": { ssid: ap }
                }
            }
        }
    });
    let yaml = serde_yaml::to_string(&doc)?;
    write_atomic(&file, yaml.as_bytes(), 0o600)?;
    if let Err(err) = apply_netplan() {
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback_path.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        cancel_rollback(unit.as_deref());
        bail!("netplan fallo al aplicar WiFi: {err}");
    }
    let (verified, verification) = verify_with_retry(iface, None);
    if !verified {
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback_path.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        cancel_rollback(unit.as_deref());
        bail!("WiFi aplicado con netplan pero la verificacion fallo:\n{verification}");
    }
    cancel_rollback(unit.as_deref());
    let _ = std::fs::remove_file(&rollback_path);
    Ok(MutationReport {
        message: format!(
            "WiFi '{}' conectado en {iface} mediante netplan.\n\n{verification}",
            ssid
        ),
        verified,
    })
}

fn netplan_set_static(
    iface: &str,
    ip_cidr: &str,
    gateway: &str,
    dns: &[String],
    backup_dir: &Path,
) -> Result<MutationReport> {
    if is_wireless(iface) {
        bail!(
            "IP fija sobre WiFi con netplan no esta soportada de forma segura; usa NetworkManager o configura el acceso WiFi primero con netplan"
        );
    }
    let renderer = netplan_renderer()?;
    if renderer.to_lowercase().contains("networkmanager") {
        bail!("netplan esta usando el renderer NetworkManager; usa platform.network_backend = \"networkmanager\"");
    }
    let file = Path::new("/etc/netplan").join(format!("90-simplebot-{iface}.yaml"));
    let rollback_path = prepare_rollback_file(&file, backup_dir, "netplan apply")?;
    let unit = schedule_rollback(&rollback_path);
    let doc = json!({
        "network": {
            "version": 2,
            "renderer": renderer,
            "ethernets": {
                iface: {
                    "dhcp4": false,
                    "addresses": [ip_cidr],
                    "routes": [{"to": "default", "via": gateway}],
                    "nameservers": {"addresses": dns}
                }
            }
        }
    });
    let yaml = serde_yaml::to_string(&doc)?;
    write_atomic(&file, yaml.as_bytes(), 0o600)?;
    if let Err(err) = apply_netplan() {
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback_path.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        cancel_rollback(unit.as_deref());
        bail!("netplan fallo al aplicar la IP fija: {err}");
    }
    let (verified, verification) = verify_with_retry(iface, Some(gateway));
    if !verified {
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback_path.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        cancel_rollback(unit.as_deref());
        bail!("la IP fija con netplan no paso la verificacion; se restauro:\n{verification}");
    }
    cancel_rollback(unit.as_deref());
    let _ = std::fs::remove_file(&rollback_path);
    Ok(MutationReport {
        message: format!(
            "IP fija aplicada con netplan en {iface}: {ip_cidr}, gateway {gateway}.\n\n{verification}"
        ),
        verified,
    })
}

fn apply_netplan() -> Result<()> {
    let generate = run_shell("netplan generate", Duration::from_secs(30), 8192, None)?;
    if !generate.success() {
        bail!("netplan generate: {}", generate.combined());
    }
    let apply = run_shell("netplan apply", Duration::from_secs(60), 8192, None)?;
    if !apply.success() {
        bail!("netplan apply: {}", apply.combined());
    }
    Ok(())
}

fn networkd_wifi_connect(
    iface: &str,
    ssid: &str,
    password: Option<&str>,
    backup_dir: &Path,
) -> Result<MutationReport> {
    let per_iface = PathBuf::from(format!("/etc/wpa_supplicant/wpa_supplicant-{iface}.conf"));
    let global = PathBuf::from("/etc/wpa_supplicant/wpa_supplicant.conf");
    let config = if per_iface.exists() {
        per_iface
    } else if global.exists() {
        global
    } else {
        per_iface
    };
    let rollback = backup_file(&config, backup_dir)?;
    let block = if let Some(password) = password.filter(|p| !p.is_empty()) {
        let pass_file = std::env::temp_dir().join(format!("simplebot-pass-{}", Uuid::new_v4()));
        write_atomic(&pass_file, password.as_bytes(), 0o600)?;
        let cmd = format!(
            "wpa_passphrase {} < {}",
            shell_quote(ssid),
            shell_quote(&pass_file.to_string_lossy())
        );
        let out = run_shell(&cmd, Duration::from_secs(20), 8192, None)?;
        let _ = std::fs::remove_file(&pass_file);
        if !out.success() {
            bail!("wpa_passphrase fallo: {}", out.combined());
        }
        out.stdout
            .lines()
            .filter(|line| !line.trim_start().starts_with("#psk"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        format!(
            "\nnetwork={{\n    ssid=\"{}\"\n    key_mgmt=NONE\n}}\n",
            ssid.replace('"', "\\\"")
        )
    };
    let mut current = if config.exists() {
        fs::read_to_string(&config).unwrap_or_default()
    } else {
        String::new()
    };
    if !current.ends_with('\n') {
        current.push('\n');
    }
    current.push_str(&block);
    if let Some(parent) = config.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&config, current.as_bytes(), 0o600)?;
    let _ = run_shell(
        &format!("wpa_cli -i {} reconfigure", shell_quote(iface)),
        Duration::from_secs(20),
        4096,
        None,
    );
    let _ = run_shell(
        &format!("systemctl restart wpa_supplicant@{}", shell_quote(iface)),
        Duration::from_secs(30),
        4096,
        None,
    );
    let _ = run_shell(
        &format!("networkctl reconfigure {}", shell_quote(iface)),
        Duration::from_secs(30),
        4096,
        None,
    );
    let (verified, verification) = verify_with_retry(iface, None);
    if !verified {
        if let Some(saved) = &rollback {
            let _ = fs::copy(saved, &config);
        } else {
            let _ = fs::remove_file(&config);
        }
        let _ = run_shell(
            &format!(
                "systemctl restart wpa_supplicant@{} ; networkctl reconfigure {}",
                shell_quote(iface),
                shell_quote(iface)
            ),
            Duration::from_secs(30),
            4096,
            None,
        );
        bail!("WiFi con wpa_supplicant no paso la verificacion:\n{verification}");
    }
    Ok(MutationReport {
        message: format!(
            "WiFi '{}' configurado en {iface} mediante wpa_supplicant/systemd-networkd.\n\n{verification}",
            ssid
        ),
        verified,
    })
}

fn networkd_set_static(
    iface: &str,
    ip_cidr: &str,
    gateway: &str,
    dns: &[String],
    backup_dir: &Path,
) -> Result<MutationReport> {
    let file = Path::new("/etc/systemd/network").join(format!("90-simplebot-{iface}.network"));
    let rollback_path = prepare_rollback_file(
        &file,
        backup_dir,
        &format!("networkctl reload && networkctl reconfigure {iface}"),
    )?;
    let unit = schedule_rollback(&rollback_path);
    let mut content = format!(
        "[Match]\nName={iface}\n\n[Network]\nDHCP=no\nAddress={ip_cidr}\nGateway={gateway}\n"
    );
    for server in dns {
        content.push_str(&format!("DNS={server}\n"));
    }
    write_atomic(&file, content.as_bytes(), 0o644)?;
    let reload = run_shell("networkctl reload", Duration::from_secs(30), 4096, None)?;
    let reconfigure = run_shell(
        &format!("networkctl reconfigure {}", shell_quote(iface)),
        Duration::from_secs(30),
        4096,
        None,
    )?;
    if !reload.success() || !reconfigure.success() {
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback_path.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        cancel_rollback(unit.as_deref());
        bail!(
            "systemd-networkd fallo: {} {}",
            reload.combined(),
            reconfigure.combined()
        );
    }
    let (verified, verification) = verify_with_retry(iface, Some(gateway));
    if !verified {
        let _ = run_shell(
            &format!("bash {}", shell_quote(&rollback_path.to_string_lossy())),
            Duration::from_secs(60),
            4096,
            None,
        );
        cancel_rollback(unit.as_deref());
        bail!("la IP fija con systemd-networkd no paso la verificacion:\n{verification}");
    }
    cancel_rollback(unit.as_deref());
    let _ = std::fs::remove_file(&rollback_path);
    Ok(MutationReport {
        message: format!(
            "IP fija aplicada con systemd-networkd en {iface}: {ip_cidr}, gateway {gateway}.\n\n{verification}"
        ),
        verified,
    })
}

fn prepare_rollback_file(file: &Path, backup_dir: &Path, reload_command: &str) -> Result<PathBuf> {
    let saved = backup_file(file, backup_dir)?;
    let stamp = timestamp().replace([':', '-'], "");
    let rollback_path = backup_dir.join(format!("{stamp}-rollback.sh"));
    let mut script = String::from("#!/bin/bash\nset -u\n");
    if let Some(saved) = &saved {
        script.push_str(&format!(
            "cp {} {}\n",
            shell_quote(&saved.to_string_lossy()),
            shell_quote(&file.to_string_lossy())
        ));
    } else {
        script.push_str(&format!("rm -f {}\n", shell_quote(&file.to_string_lossy())));
    }
    script.push_str(reload_command);
    script.push('\n');
    write_rollback_script(&rollback_path, &script)
}

fn write_rollback_script(path: &Path, content: &str) -> Result<PathBuf> {
    write_atomic(path, content.as_bytes(), 0o700)?;
    Ok(path.to_path_buf())
}

fn schedule_rollback(script: &Path) -> Option<String> {
    let unit = format!("simplebot-rollback-{}", &Uuid::new_v4().to_string()[..8]);
    let command = format!(
        "systemd-run --unit={} --on-active=180 {}",
        unit,
        shell_quote(&script.to_string_lossy())
    );
    let out = run_shell(&command, Duration::from_secs(20), 4096, None).ok()?;
    if out.success() {
        Some(unit)
    } else {
        log::warn!(
            "no se pudo programar el rollback automatico: {}",
            out.combined()
        );
        None
    }
}

fn cancel_rollback(unit: Option<&str>) {
    if let Some(unit) = unit {
        let _ = run_shell(
            &format!("systemctl stop {} ; systemctl reset-failed {}", unit, unit),
            Duration::from_secs(15),
            2048,
            None,
        );
    }
}

fn keyfile_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_route_with_gateway() {
        let (iface, gw) =
            parse_default_route("default via 192.168.1.1 dev wlan0 proto dhcp metric 600\n");
        assert_eq!(iface.as_deref(), Some("wlan0"));
        assert_eq!(gw.as_deref(), Some("192.168.1.1"));
    }

    #[test]
    fn escapes_keyfile_values() {
        assert_eq!(keyfile_escape("a\\b\nc"), "a\\\\b\\nc");
    }

    #[test]
    fn interfaces_do_not_panic() {
        let _ = wireless_interfaces();
    }
}
