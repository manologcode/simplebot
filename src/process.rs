use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;
use wait_timeout::ChildExt;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub duration_ms: u128,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        !self.timed_out && self.status == Some(0)
    }

    pub fn combined(&self) -> String {
        let mut text = String::new();
        if !self.stdout.is_empty() {
            text.push_str(&self.stdout);
        }
        if !self.stderr.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("[stderr]\n");
            text.push_str(&self.stderr);
        }
        text
    }
}

pub fn run_shell(
    command: &str,
    timeout: Duration,
    max_bytes: usize,
    cwd: Option<&Path>,
) -> Result<CommandOutput> {
    let id = Uuid::new_v4();
    let out_path = std::env::temp_dir().join(format!("simplebot-{id}.out"));
    let err_path = std::env::temp_dir().join(format!("simplebot-{id}.err"));
    let out_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&out_path)
        .context("no se pudo crear salida temporal")?;
    let err_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&err_path)
        .context("no se pudo crear error temporal")?;

    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd.unwrap_or_else(|| Path::new("/")))
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file));
    configure_environment(&mut cmd);
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .with_context(|| format!("no se pudo ejecutar: {command}"))?;
    let pid = child.id() as i32;
    let status = child.wait_timeout(timeout)?;
    let timed_out = status.is_none();
    if timed_out {
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(300));
        if child.try_wait()?.is_none() {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.wait();
    }
    let duration_ms = started.elapsed().as_millis();
    let exit = if timed_out {
        None
    } else {
        status.unwrap().code()
    };

    let stdout = read_limit(&out_path, max_bytes).unwrap_or_default();
    let stderr = read_limit(&err_path, max_bytes).unwrap_or_default();
    let _ = fs::remove_file(out_path);
    let _ = fs::remove_file(err_path);

    Ok(CommandOutput {
        status: exit,
        stdout,
        stderr,
        timed_out,
        duration_ms,
    })
}

fn configure_environment(cmd: &mut Command) {
    cmd.env_clear();
    cmd.env(
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    );
    cmd.env("HOME", "/root");
    cmd.env("USER", "root");
    cmd.env("LOGNAME", "root");
    cmd.env("LANG", "C.UTF-8");
    cmd.env("LC_ALL", "C");
    cmd.env("DEBIAN_FRONTEND", "noninteractive");
    cmd.env("SIMPLEBOT", "1");
    for key in [
        "TERM",
        "SSH_CONNECTION",
        "SSH_TTY",
        "DBUS_SYSTEM_BUS_ADDRESS",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
}

fn read_limit(path: &Path, max_bytes: usize) -> Result<String> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    if len <= max_bytes {
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        return Ok(String::from_utf8_lossy(&buf).replace('\0', ""));
    }
    let head_len = max_bytes / 2;
    let tail_len = max_bytes.saturating_sub(head_len);
    let mut head = Vec::new();
    Read::by_ref(&mut file)
        .take(head_len as u64)
        .read_to_end(&mut head)?;
    if tail_len > 0 {
        file.seek(SeekFrom::End(-(tail_len as i64)))?;
    }
    let mut tail = Vec::new();
    Read::by_ref(&mut file)
        .take(tail_len as u64)
        .read_to_end(&mut tail)?;
    let omitted = len.saturating_sub(head.len() + tail.len());
    Ok(format!(
        "{}\n... [{omitted} bytes omitidos] ...\n{}",
        String::from_utf8_lossy(&head).replace('\0', ""),
        String::from_utf8_lossy(&tail).replace('\0', "")
    ))
}

pub fn truncate_middle(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    if max_bytes < 128 {
        return text.chars().take(max_bytes).collect();
    }
    let head_len = max_bytes / 2;
    let tail_len = max_bytes - head_len;
    let mut head = String::new();
    for ch in text.chars() {
        if head.len() + ch.len_utf8() > head_len {
            break;
        }
        head.push(ch);
    }
    let mut tail_chars: Vec<char> = text.chars().rev().take(tail_len).collect();
    tail_chars.reverse();
    let tail: String = tail_chars.into_iter().collect();
    let omitted = text.len().saturating_sub(head.len() + tail.len());
    format!("{head}\n... [{omitted} bytes omitidos] ...\n{tail}")
}

pub fn run_readonly(command: &str, max_bytes: usize) -> Result<CommandOutput> {
    run_shell(command, Duration::from_secs(20), max_bytes, None)
}

pub fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn command_exists(binary: &str) -> bool {
    if binary.contains('/') {
        return Path::new(binary).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(binary);
        candidate
            .metadata()
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

pub fn command_path(binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        return Path::new(binary).is_file().then(|| PathBuf::from(binary));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| {
            candidate
                .metadata()
                .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn is_ssh_session() -> bool {
    for key in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "SSH_AUTH_SOCK"] {
        if std::env::var(key)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            return true;
        }
    }
    let mut pid = std::process::id();
    for _ in 0..16 {
        let status = match fs::read_to_string(format!("/proc/{pid}/status")) {
            Ok(text) => text,
            Err(_) => break,
        };
        let name = status
            .lines()
            .find_map(|line| line.strip_prefix("Name:\t"))
            .unwrap_or("")
            .to_lowercase();
        if name.contains("sshd") || name.contains("dropbear") || name.contains("mosh-server") {
            return true;
        }
        let ppid = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:\t"))
            .and_then(|value| value.trim().parse::<u32>().ok());
        match ppid {
            Some(parent) if parent > 1 && parent != pid => pid = parent,
            _ => break,
        }
    }
    false
}

pub fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub fn compact_timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

pub fn write_atomic(path: &Path, content: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().context("la ruta no tiene directorio padre")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".simplebot-tmp-{}", Uuid::new_v4()));
    {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .with_context(|| format!("no se pudo crear {}", temp.display()))?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    fs::set_permissions(&temp, fs::Permissions::from_mode(mode))?;
    fs::rename(&temp, path).with_context(|| format!("no se pudo reemplazar {}", path.display()))?;
    Ok(())
}

pub fn backup_file(path: &Path, backup_dir: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = compact_timestamp();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archivo".to_string());
    let dest_dir = backup_dir.join(stamp);
    fs::create_dir_all(&dest_dir)?;
    let _ = fs::set_permissions(&dest_dir, fs::Permissions::from_mode(0o700));
    let mut dest = dest_dir.join(&name);
    let mut counter = 1;
    while dest.exists() {
        dest = dest_dir.join(format!("{name}.{counter}"));
        counter += 1;
    }
    fs::copy(path, &dest)?;
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&dest, meta.permissions());
    }
    Ok(Some(dest))
}

pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.{ext}"));
    let mut counter = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}-{counter}.{ext}"));
        counter += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_keeping_ends() {
        let text = format!("{}MEDIO{}", "a".repeat(500), "z".repeat(500));
        let out = truncate_middle(&text, 200);
        assert!(out.len() < 400);
        assert!(out.starts_with("aaa"));
        assert!(out.ends_with("zzz"));
        assert!(out.contains("omitidos"));
    }

    #[test]
    fn short_text_is_untouched() {
        assert_eq!(truncate_middle("hola", 100), "hola");
    }

    #[test]
    fn runs_echo() {
        let out = run_shell("echo hola", Duration::from_secs(5), 1000, None).unwrap();
        assert!(out.success());
        assert!(out.stdout.contains("hola"));
    }

    #[test]
    fn caps_large_command_output() {
        let out = run_shell("seq 1 200000", Duration::from_secs(10), 1000, None).unwrap();
        assert!(out.success());
        assert!(out.stdout.len() < 1500);
        assert!(out.stdout.contains("omitidos"));
    }

    #[test]
    fn detects_timeout() {
        let out = run_shell("sleep 3", Duration::from_millis(300), 1000, None).unwrap();
        assert!(out.timed_out);
        assert!(!out.success());
    }

    #[test]
    fn finds_bash() {
        assert!(command_exists("bash"));
    }
}
