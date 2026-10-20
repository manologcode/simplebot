pub mod redact;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    Safe,
    Low,
    Medium,
    High,
    Blocked,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Safe => "lectura",
            Risk::Low => "bajo",
            Risk::Medium => "medio",
            Risk::High => "alto",
            Risk::Blocked => "bloqueado",
        }
    }
}

pub fn classify(command: &str) -> Risk {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Risk::Medium;
    }
    let lower = trimmed.to_lowercase();
    let normalized = lower.trim_start();
    let normalized = normalized
        .strip_prefix("sudo ")
        .unwrap_or(normalized)
        .trim_start();

    if is_blocked(normalized) {
        return Risk::Blocked;
    }
    if is_high(normalized) {
        return Risk::High;
    }
    if is_safe_command(normalized) {
        return Risk::Safe;
    }
    if is_medium(normalized) {
        return Risk::Medium;
    }
    Risk::Medium
}

fn is_blocked(cmd: &str) -> bool {
    let compact: String = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    let blocked_fragments = [
        "mkfs",
        "wipefs",
        "shred /dev/",
        "dd if=/dev/zero of=/dev/",
        "dd if=/dev/random of=/dev/",
        "--no-preserve-root",
        "find / -delete",
        "find /* -delete",
        "find / -exec",
        "find /* -exec",
        "xargs rm",
        "chmod -r 777 /",
        "chown -r",
        ":(){",
        "mv / /",
        "mv /* /dev/null",
        "> /dev/sd",
        "of=/dev/sd",
        "of=/dev/nvme",
        "of=/dev/mmcblk",
        "init 0",
        "systemctl poweroff",
        "systemctl reboot",
    ];
    if blocked_fragments.iter().any(|f| compact.contains(f)) {
        return true;
    }
    let dd_re = Regex::new(r"(^|\s)dd\s+.*of=/dev/").unwrap();
    if dd_re.is_match(cmd) {
        return true;
    }
    let rm_root = Regex::new(
        r"(^|\s)rm\s+(-[^\s]*\s+)*(/(\*)?|/(etc|usr|var|boot|root|home|bin|sbin|lib|lib64|opt|srv|dev|proc|sys)(/\*?)?)(\s|$)",
    )
    .unwrap();
    rm_root.is_match(cmd)
}

fn is_high(cmd: &str) -> bool {
    let fragments = [
        "apt install",
        "apt-get install",
        "aptitude install",
        "apt remove",
        "apt-get remove",
        "apt purge",
        "apt-get purge",
        "apt-get upgrade",
        "apt-get dist-upgrade",
        "apt full-upgrade",
        "snap install",
        "snap remove",
        "pip install",
        "pip3 install",
        "npm install -g",
        "cargo install",
        "dpkg -i",
        "dpkg --install",
        "dpkg -r",
        "curl | sh",
        "curl | bash",
        "wget | sh",
        "wget | bash",
        "curl -s | sh",
        "bash -c",
        "sh -c",
        "eval ",
        "reboot",
        "shutdown",
        "poweroff",
        "halt",
        "init 6",
        "rm ",
        "rmdir ",
        "userdel",
        "groupdel",
        "passwd ",
        "visudo",
        "crontab -r",
        "iptables -f",
        "nft flush",
        "ufw disable",
        "systemctl disable",
        "systemctl mask",
        "mount ",
        "umount ",
        "mkswap",
        "swapon",
        "swapoff",
        "hostnamectl set",
        "timedatectl set",
        "tee /etc/",
        "tee /boot/",
        "sed -i",
        "truncate ",
        "> /etc/",
        ">> /etc/",
        "> /boot/",
        ">> /boot/",
        "chmod -r",
        "chown ",
        "killall",
        "pkill",
        "kill -9",
    ];
    fragments.iter().any(|f| cmd.contains(f))
}

fn is_medium(cmd: &str) -> bool {
    let fragments = [
        "nmcli con",
        "nmcli connection",
        "nmcli device",
        "nmcli networking",
        "nmcli radio",
        "netplan apply",
        "netplan try",
        "netplan set",
        "networkctl reload",
        "networkctl reconfigure",
        "systemctl restart",
        "systemctl stop",
        "systemctl start",
        "systemctl enable",
        "systemctl reload",
        "service ",
        "ip addr add",
        "ip addr del",
        "ip route add",
        "ip route del",
        "ip link set",
        "ip link add",
        "ip link del",
        "wpa_cli",
        "wpa_supplicant",
        "rfkill block",
        "rfkill unblock",
        "apt-get update",
        "apt update",
        "mkdir ",
        "touch ",
        "cp ",
        "mv ",
        "ln -s",
        "chmod ",
        "chown ",
        "useradd",
        "groupadd",
        "usermod",
        "systemd-run",
        "at ",
        "tee ",
        "sed ",
    ];
    fragments.iter().any(|f| cmd.contains(f))
}

fn is_safe_command(cmd: &str) -> bool {
    if cmd.contains('>')
        || cmd.contains('<')
        || cmd.contains('`')
        || cmd.contains("$(")
        || cmd.contains("${")
    {
        return false;
    }
    if cmd.replace("&&", "").contains('&') {
        return false;
    }
    let parts: Vec<&str> = cmd
        .split(['|', ';', '\n'])
        .flat_map(|part| part.split("&&"))
        .flat_map(|part| part.split("||"))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return false;
    }
    parts.iter().all(|part| is_safe_simple(part))
}

fn is_safe_simple(part: &str) -> bool {
    let mut words = part.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let rest: Vec<String> = words.map(|w| w.to_lowercase()).collect();
    let first_non_option = || {
        rest.iter()
            .find(|w| !w.starts_with('-'))
            .map(String::as_str)
    };
    match program {
        "ls" | "cat" | "head" | "tail" | "grep" | "wc" | "cut" | "tr" | "df" | "du" | "free"
        | "uname" | "uptime" | "whoami" | "id" | "groups" | "ps" | "pwd" | "stat" | "file"
        | "readlink" | "realpath" | "which" | "sha256sum" | "sha1sum" | "md5sum" | "lsblk"
        | "lscpu" | "lspci" | "lsusb" | "sensors" | "vcgencmd" | "ping" | "getent" | "ss"
        | "netstat" | "mtr" | "apt-cache" | "printf" | "echo" | "test" | "true" | "false"
        | "sleep" | "basename" | "dirname" => true,
        "date" => !rest
            .iter()
            .any(|w| w == "-s" || w == "--set" || w.starts_with("--set=")),
        "dmesg" => !rest.iter().any(|w| {
            matches!(
                w.as_str(),
                "-c" | "-C" | "--clear" | "-n" | "--console-level"
            )
        }),
        "resolvectl" => matches!(
            first_non_option(),
            None | Some("status")
                | Some("query")
                | Some("statistics")
                | Some("dns")
                | Some("domain")
                | Some("default-route")
                | Some("llmnr")
                | Some("mdns")
                | Some("show-cache")
        ),
        "find" => !rest.iter().any(|w| {
            w == "-delete"
                || w == "-exec"
                || w == "-execdir"
                || w == "-ok"
                || w == "-okdir"
                || w == "-fprint"
                || w == "-fprintf"
                || w == "-fls"
        }),
        "apt-get" | "apt" => rest.first().is_some_and(|w| w == "-s" || w == "simulate"),
        "nmcli" => {
            let mutating = [
                "add",
                "modify",
                "delete",
                "up",
                "down",
                "connect",
                "disconnect",
                "reload",
                "on",
                "off",
                "set",
                "create",
                "remove",
                "clone",
            ];
            let objects = [
                "general",
                "device",
                "connection",
                "con",
                "networking",
                "radio",
                "monitor",
            ];
            !rest.iter().any(|w| mutating.contains(&w.as_str()))
                && rest.iter().any(|w| objects.contains(&w.as_str()))
        }
        "ip" => {
            let mutating = [
                "add", "del", "delete", "set", "flush", "change", "replace", "append",
            ];
            !rest.iter().any(|w| mutating.contains(&w.as_str()))
        }
        "systemctl" => matches!(
            first_non_option(),
            Some("status")
                | Some("show")
                | Some("is-active")
                | Some("is-enabled")
                | Some("is-failed")
                | Some("list-units")
                | Some("list-unit-files")
                | Some("cat")
        ),
        "networkctl" => matches!(first_non_option(), None | Some("status") | Some("list")),
        "netplan" => matches!(first_non_option(), Some("get")),
        "wpa_cli" => matches!(
            first_non_option(),
            Some("status") | Some("scan_results") | Some("list_networks") | Some("ping")
        ),
        "hostnamectl" | "timedatectl" => rest.is_empty() || first_non_option() == Some("status"),
        "dpkg" => matches!(
            first_non_option(),
            Some("-l") | Some("-s") | Some("-L") | Some("status")
        ),
        "journalctl" => !rest.iter().any(|w| {
            w == "--rotate"
                || w == "--flush"
                || w == "--sync"
                || w.starts_with("--vacuum-size")
                || w.starts_with("--vacuum-time")
                || w.starts_with("--vacuum-files")
        }),
        "rfkill" => first_non_option() == Some("list"),
        "command" => matches!(rest.first().map(String::as_str), Some("-v") | Some("-V")),
        _ => false,
    }
}

pub fn is_sensitive_path(path: &str) -> bool {
    let p = path.trim();
    [
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/sudoers",
        "/etc/sudoers.d/",
        "/root/.ssh/",
        "/etc/ssh/",
        "/etc/ssl/private/",
        "/etc/NetworkManager/system-connections/",
        "/var/lib/NetworkManager/",
        "/etc/wpa_supplicant/",
        "/root/.config/simplebot/api_key",
        "/etc/simplebot/api_key",
    ]
    .iter()
    .any(|prefix| p == *prefix || p.starts_with(prefix))
        || p.contains("/.ssh")
        || p.ends_with(".key")
        || p.ends_with(".pem")
        || p.ends_with("id_rsa")
        || p.ends_with("id_ed25519")
}

pub fn contains_sensitive_path(command: &str) -> bool {
    [
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/sudoers",
        "/root",
        "/root/.ssh",
        "/.ssh",
        "/etc/ssl/private",
        "/etc/NetworkManager/system-connections",
        "/var/lib/NetworkManager",
        "/etc/wpa_supplicant",
        "/etc/simplebot/api_key",
    ]
    .iter()
    .any(|fragment| command.contains(fragment))
}

pub fn is_install_command(command: &str) -> bool {
    let lower = command.to_lowercase();
    let lower = lower.trim_start().strip_prefix("sudo ").unwrap_or(&lower);
    let normalized = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    ["apt install", "apt-get install", "aptitude install"]
        .iter()
        .any(|p| normalized.contains(p))
}

pub fn is_critical_path(path: &str) -> bool {
    let p = path.trim();
    [
        "/etc/fstab",
        "/etc/passwd",
        "/etc/shadow",
        "/etc/sudoers",
        "/etc/ssh/",
        "/boot/",
        "/etc/default/grub",
        "/etc/network/interfaces",
        "/etc/systemd/system/",
        "/lib/systemd/system/",
    ]
    .iter()
    .any(|prefix| p == *prefix || p.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_safe() {
        assert_eq!(classify("ip -brief addr"), Risk::Safe);
        assert_eq!(
            classify("journalctl -u NetworkManager -n 50 --no-pager"),
            Risk::Safe
        );
        assert_eq!(classify("cat /etc/os-release"), Risk::Safe);
        assert_eq!(classify("dmesg | tail -n 30"), Risk::Safe);
        assert_eq!(
            classify("nmcli -t -f DEVICE,STATE device status"),
            Risk::Safe
        );
        assert_eq!(classify("nmcli device wifi list"), Risk::Safe);
        assert_eq!(classify("netplan get"), Risk::Safe);
        assert_eq!(classify("wpa_cli status"), Risk::Safe);
        assert_eq!(classify("date"), Risk::Safe);
        assert_eq!(classify("resolvectl status"), Risk::Safe);
    }

    #[test]
    fn read_only_commands_that_can_mutate_are_not_safe() {
        for command in [
            "ifconfig eth0 down",
            "route add default gw 1.1.1.1",
            "iw dev wlan0 connect casa",
            "sort -o /etc/passwd /tmp/x",
            "awk 'BEGIN{system(\"id\")}'",
            "sed -i s/a/b/ /etc/hostname",
            "dmesg -C",
            "resolvectl flush-caches",
            "date -s 2020-01-01",
        ] {
            assert_ne!(classify(command), Risk::Safe, "{command}");
        }
    }

    #[test]
    fn mutations_are_not_safe() {
        assert_eq!(
            classify("nmcli connection modify foo ipv4.method manual"),
            Risk::Medium
        );
        assert_eq!(classify("systemctl restart NetworkManager"), Risk::Medium);
        assert_eq!(classify("mkdir /tmp/x"), Risk::Medium);
    }

    #[test]
    fn installs_are_high() {
        assert_eq!(classify("apt-get install -y htop"), Risk::High);
        assert!(is_install_command("apt-get install -y htop"));
    }

    #[test]
    fn dangerous_is_blocked() {
        assert_eq!(classify("rm -rf /"), Risk::Blocked);
        assert_eq!(classify("dd if=/dev/zero of=/dev/sda bs=1M"), Risk::Blocked);
        assert_eq!(classify("mkfs.ext4 /dev/sda1"), Risk::Blocked);
        assert_eq!(classify("rm -rf --no-preserve-root /"), Risk::Blocked);
        assert_eq!(classify("find / -delete"), Risk::Blocked);
        assert_eq!(classify("find / -exec rm -rf {} +"), Risk::Blocked);
        assert_eq!(classify("xargs rm"), Risk::Blocked);
    }

    #[test]
    fn unknown_requires_confirmation() {
        assert_eq!(classify("frobnicate --all"), Risk::Medium);
    }

    #[test]
    fn sensitive_paths() {
        assert!(is_sensitive_path("/etc/shadow"));
        assert!(is_sensitive_path("/root/.ssh/id_ed25519"));
        assert!(is_sensitive_path("/home/pi/.ssh/id_ed25519"));
        assert!(!is_sensitive_path("/etc/hostname"));
        assert!(contains_sensitive_path("cat /etc/shadow"));
        assert!(!contains_sensitive_path("cat /etc/hostname"));
    }

    #[test]
    fn critical_paths() {
        assert!(is_critical_path("/etc/ssh/sshd_config"));
        assert!(is_critical_path("/boot/config.txt"));
        assert!(!is_critical_path("/etc/hostname"));
    }
}
