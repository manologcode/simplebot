use regex::Regex;

pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if secret.len() >= 3 {
            out = out.replace(secret, "[REDACTED]");
        }
    }
    for pattern in [
        r#"(?i)((?:password|passwd|psk|secret|token|api[_-]?key)\s*["']?\s*[:=]\s*["']?)([^"'\s,;}]+)"#,
        r"(?i)(authorization:\s*bearer\s+)(\S+)",
        r"(?i)(--password[=\s]+)(\S+)",
        r"(?i)((?:password|passwd|psk)\s+)(\S+)",
        r"(?i)(OPENCODE_API_KEY=)(\S+)",
    ] {
        if let Ok(re) = Regex::new(pattern) {
            out = re
                .replace_all(&out, |caps: &regex::Captures<'_>| {
                    format!("{}[REDACTED]", &caps[1])
                })
                .to_string();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_exact_secrets() {
        let out = redact(
            "clave super-secreta-123 fin",
            &["super-secreta-123".to_string()],
        );
        assert!(!out.contains("super-secreta-123"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn hides_assignments() {
        let out = redact("password=abc123 token: xyz", &[]);
        assert!(!out.contains("abc123"));
        assert!(!out.contains("xyz"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn hides_json_values() {
        let out = redact(r#"{"password": "abc123", "ssid": "casa"}"#, &[]);
        assert!(!out.contains("abc123"));
        assert!(out.contains("casa"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn hides_command_line_passwords() {
        let out = redact("nmcli device wifi connect casa password abc123", &[]);
        assert!(!out.contains("abc123"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn keeps_secret_references() {
        let text = "usa {{secretref-1}} y {{ secretref-2 }}";
        assert_eq!(redact(text, &[]), text);
    }

    #[test]
    fn keeps_normal_text() {
        assert_eq!(redact("conectado a wlan0", &[]), "conectado a wlan0");
    }
}
