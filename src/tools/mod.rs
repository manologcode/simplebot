use crate::audit::AuditLogger;
use crate::config::{Config, GenerationMode, InstallPolicy, PermissionMode};
use crate::platform::network::{self, ssh_risk_for};
use crate::platform::packages;
use crate::platform::PlatformInfo;
use crate::process::{
    backup_file, command_exists, is_root, run_readonly, run_shell, shell_quote, timestamp,
    truncate_middle, write_atomic,
};
use crate::provider::ToolSpec;
use crate::safety::{
    classify, contains_sensitive_path, is_critical_path, is_install_command, is_sensitive_path,
    redact::redact, Risk,
};
use crate::secrets::SecretStore;
use crate::skills::{self, SkillRegistry};
use crate::ui::Ui;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize)]
pub struct FinishCall {
    pub success: bool,
    pub summary: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolResult {
    pub content: String,
    pub success: bool,
    pub mutated: bool,
    pub verified: bool,
    pub finish: Option<FinishCall>,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: true,
            mutated: false,
            verified: false,
            finish: None,
        }
    }

    pub fn failure(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: false,
            mutated: false,
            verified: false,
            finish: None,
        }
    }

    pub fn mutation(content: impl Into<String>, verified: bool) -> Self {
        Self {
            content: content.into(),
            success: true,
            mutated: true,
            verified,
            finish: None,
        }
    }

    pub fn finish(call: FinishCall) -> Self {
        Self {
            content: format!(
                "finish recibido: success={} summary={} evidencia={}",
                call.success,
                call.summary,
                call.evidence.len()
            ),
            success: true,
            mutated: false,
            verified: false,
            finish: Some(call),
        }
    }
}

pub struct ToolContext<'a> {
    pub config: &'a Config,
    pub platform: &'a PlatformInfo,
    pub skills: &'a SkillRegistry,
    pub ui: &'a mut Ui,
    pub secrets: &'a mut SecretStore,
    pub loaded_skills: &'a mut Vec<String>,
    pub audit: &'a AuditLogger,
    pub session_id: &'a str,
}

pub fn execute_tool(ctx: &mut ToolContext<'_>, name: &str, args: Value) -> ToolResult {
    let started = Instant::now();
    let secret_values = ctx.secrets.values();
    let args_text = redact(&args.to_string(), &secret_values);
    let risk = if name == "run_command" || name == "verify" {
        args.get("command")
            .and_then(Value::as_str)
            .map(classify)
            .unwrap_or(Risk::Safe)
    } else {
        Risk::Safe
    };

    let mutating_tool = matches!(
        name,
        "write_file" | "install_packages" | "wifi_connect" | "set_static_ip" | "save_skill"
    );
    let result = if mutating_tool && ctx.config.permissions.mode == PermissionMode::ReadOnly {
        ToolResult::failure(
            "El modo read_only impide cambios. Cambia permissions.mode en la configuracion.",
        )
    } else {
        match name {
            "run_command" => tool_run_command(ctx, &args),
            "read_file" => tool_read_file(ctx, &args),
            "list_dir" => tool_list_dir(&args),
            "write_file" => tool_write_file(ctx, &args),
            "ask_user" => tool_ask_user(ctx, &args),
            "collect_logs" => tool_collect_logs(ctx, &args),
            "verify" => tool_verify(ctx, &args),
            "install_packages" => tool_install_packages(ctx, &args),
            "network_status" => tool_network_status(ctx),
            "wifi_connect" => tool_wifi_connect(ctx, &args),
            "set_static_ip" => tool_set_static_ip(ctx, &args),
            "skill" => tool_load_skill(ctx, &args),
            "save_skill" => tool_save_skill(ctx, &args),
            "finish" => tool_finish(&args),
            other => Err(anyhow::anyhow!("herramienta desconocida: {other}")),
        }
        .unwrap_or_else(|err| ToolResult::failure(format!("Error: {err:#}")))
    };

    ctx.audit.log(json!({
        "ts": timestamp(),
        "session": ctx.session_id,
        "tool": name,
        "args": args_text,
        "risk": risk.label(),
        "success": result.success,
        "mutated": result.mutated,
        "verified": result.verified,
        "duration_ms": started.elapsed().as_millis(),
    }));
    result
}

pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        spec(
            "run_command",
            "Ejecuta un comando de shell como root. Usalo para tareas no cubiertas por otras herramientas. Devuelve stdout, stderr y codigo de salida. Despues de un cambio, verifica con verify.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Comando a ejecutar"},
                    "purpose": {"type": "string", "description": "Para que se ejecuta"},
                    "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 600}
                },
                "required": ["command", "purpose"]
            }),
        ),
        spec(
            "read_file",
            "Lee un fichero de texto del sistema, con limite de bytes.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": 1000000}
                },
                "required": ["path"]
            }),
        ),
        spec(
            "list_dir",
            "Lista el contenido de un directorio.",
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        ),
        spec(
            "write_file",
            "Escribe un fichero de texto con backup automatico. Los ficheros criticos requieren confirmacion.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "reason": {"type": "string"}
                },
                "required": ["path", "content", "reason"]
            }),
        ),
        spec(
            "ask_user",
            "Pide un dato al usuario. Usa kind=secret para contrasenas: devuelve una referencia secretref-N que debes usar como {{secretref-N}} en run_command, sin escribir el secreto en claro.",
            json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string"},
                    "kind": {"type": "string", "enum": ["text", "choice", "secret"]},
                    "options": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["question", "kind"]
            }),
        ),
        spec(
            "collect_logs",
            "Recoge logs acotados del sistema o de una unidad systemd.",
            json!({
                "type": "object",
                "properties": {
                    "unit": {"type": "string"},
                    "lines": {"type": "integer", "minimum": 1, "maximum": 500}
                }
            }),
        ),
        spec(
            "verify",
            "Ejecuta un comando de solo lectura para comprobar el estado. Solo acepta comandos seguros. Devuelve exito o fallo y la salida.",
            json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }),
        ),
        spec(
            "install_packages",
            "Instala paquetes con apt. Siempre pasa por la politica de permisos y pide confirmacion segun la configuracion.",
            json!({
                "type": "object",
                "properties": {
                    "packages": {"type": "array", "items": {"type": "string"}},
                    "reason": {"type": "string"}
                },
                "required": ["packages", "reason"]
            }),
        ),
        spec(
            "network_status",
            "Muestra el estado de la red, interfaces, rutas, DNS, WiFi y backend detectado.",
            json!({"type": "object", "properties": {}}),
        ),
        spec(
            "wifi_connect",
            "Conecta a una red WiFi usando el backend detectado. La contrasena debe venir como referencia secretref-N obtenida con ask_user(kind=secret).",
            json!({
                "type": "object",
                "properties": {
                    "ssid": {"type": "string"},
                    "password_ref": {"type": "string", "description": "Referencia secretref-N"},
                    "password": {"type": "string", "description": "Solo si el usuario la dio explicitamente"},
                    "hidden": {"type": "boolean"},
                    "interface": {"type": "string"}
                },
                "required": ["ssid"]
            }),
        ),
        spec(
            "set_static_ip",
            "Configura una IP fija y verifica conectividad. Si la sesion SSH usa esa interfaz, exige confirmacion y programa rollback.",
            json!({
                "type": "object",
                "properties": {
                    "interface": {"type": "string"},
                    "ip_cidr": {"type": "string"},
                    "gateway": {"type": "string"},
                    "dns": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["interface", "ip_cidr", "gateway"]
            }),
        ),
        spec(
            "skill",
            "Carga las instrucciones completas de una skill disponible por nombre.",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        ),
        spec(
            "save_skill",
            "Crea o actualiza una skill SKILL.md a partir de pasos verificados. Muestra el contenido y pide confirmacion.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "description": {"type": "string"},
                    "instructions": {"type": "string"},
                    "compatibility": {"type": "string"},
                    "overwrite": {"type": "boolean"}
                },
                "required": ["name", "description", "instructions"]
            }),
        ),
        spec(
            "finish",
            "Finaliza la tarea. Si hubo cambios, solo se acepta success=true despues de una verificacion exitosa.",
            json!({
                "type": "object",
                "properties": {
                    "success": {"type": "boolean"},
                    "summary": {"type": "string"},
                    "evidence": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["success", "summary", "evidence"]
            }),
        ),
    ]
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

fn tool_run_command(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let command = arg_str(args, "command")?;
    let purpose = arg_str(args, "purpose")?;
    let timeout = args
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(120)
        .clamp(1, 600);
    let mut risk = classify(&command);
    if contains_sensitive_path(&command) && risk == Risk::Safe {
        risk = Risk::High;
    }
    if is_install_command(&command) {
        return Ok(ToolResult::failure(
            "Para instalar con apt usa la herramienta install_packages. Para otros gestores (pip, npm, cargo), ejecuta el comando con run_command y su confirmacion.",
        ));
    }
    if !authorize(ctx, risk, &command, &purpose)? {
        return Ok(ToolResult::failure("Operacion cancelada por el usuario."));
    }
    let resolved = ctx.secrets.resolve(&command)?;
    if ctx.ui.dry_run {
        return Ok(ToolResult {
            content: format!(
                "DRY-RUN: se ejecutaria (riesgo {}): {}\nProposito: {}",
                risk.label(),
                command,
                purpose
            ),
            success: true,
            mutated: false,
            verified: false,
            finish: None,
        });
    }
    let output = run_shell(
        &resolved,
        Duration::from_secs(timeout),
        ctx.config.agent.max_tool_output_bytes,
        None,
    )?;
    let secret_values = ctx.secrets.values();
    let content = redact(
        &format!(
            "exit={:?} timed_out={} duracion={}ms\n{}",
            output.status,
            output.timed_out,
            output.duration_ms,
            output.combined()
        ),
        &secret_values,
    );
    Ok(ToolResult {
        content,
        success: output.success(),
        mutated: risk != Risk::Safe,
        verified: false,
        finish: None,
    })
}

fn tool_verify(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let command = arg_str(args, "command")?;
    let risk = classify(&command);
    if risk != Risk::Safe {
        return Ok(ToolResult::failure(format!(
            "verify solo acepta comandos de solo lectura; '{}' se clasifico como {}",
            command,
            risk.label()
        )));
    }
    let output = run_readonly(&command, ctx.config.agent.max_tool_output_bytes)?;
    let secret_values = ctx.secrets.values();
    let content = redact(
        &format!(
            "VERIFICACION {} (exit={:?})\n{}",
            if output.success() { "OK" } else { "FALLIDA" },
            output.status,
            output.combined()
        ),
        &secret_values,
    );
    Ok(ToolResult {
        content,
        success: output.success(),
        mutated: false,
        verified: output.success(),
        finish: None,
    })
}

fn tool_read_file(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let path = expand_path(&arg_str(args, "path")?);
    let check_path = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if is_sensitive_path(&check_path.to_string_lossy()) {
        return Ok(ToolResult::failure(format!(
            "Por seguridad no se permite leer el fichero sensible {}",
            check_path.display()
        )));
    }
    let max = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(64_000)
        .clamp(1, 1_000_000) as usize;
    let meta =
        fs::metadata(&path).with_context(|| format!("no se pudo leer {}", path.display()))?;
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .with_context(|| format!("no se pudo abrir {}", path.display()))?
        .take(max as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("no se pudo leer {}", path.display()))?;
    let mut text = String::from_utf8_lossy(&bytes).to_string();
    if meta.len() > max as u64 {
        text.push_str(&format!(
            "\n... [truncado: {} bytes de {} omitidos] ...",
            meta.len().saturating_sub(max as u64),
            meta.len()
        ));
    }
    let secret_values = ctx.secrets.values();
    Ok(ToolResult::ok(redact(
        &format!("Contenido de {}:\n{}", path.display(), text),
        &secret_values,
    )))
}

fn tool_list_dir(args: &Value) -> Result<ToolResult> {
    let path = expand_path(&arg_str(args, "path")?);
    let mut entries = Vec::new();
    for entry in fs::read_dir(&path)
        .with_context(|| format!("no se pudo listar {}", path.display()))?
        .flatten()
        .take(300)
    {
        let kind = if entry.path().is_dir() { "dir" } else { "file" };
        entries.push(format!("{kind}\t{}", entry.file_name().to_string_lossy()));
    }
    entries.sort();
    Ok(ToolResult::ok(format!(
        "Contenido de {} ({} entradas):\n{}",
        path.display(),
        entries.len(),
        entries.join("\n")
    )))
}

fn tool_write_file(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let path = expand_path(&arg_str(args, "path")?);
    let content = arg_str(args, "content")?;
    let reason = arg_str(args, "reason")?;
    let check_path = effective_write_path(&path);
    let critical = is_critical_path(&check_path.to_string_lossy())
        || is_sensitive_path(&check_path.to_string_lossy());
    if critical || ctx.config.permissions.mode == PermissionMode::ConfirmAll {
        let ok = if critical {
            ctx.ui.danger_confirm(&format!(
                "Se va a escribir en un fichero critico: {}\nMotivo: {}",
                path.display(),
                reason
            ))
        } else {
            ctx.ui.confirm(
                &format!("Escribir {} (motivo: {})?", path.display(), reason),
                false,
            )
        };
        if !ok {
            return Ok(ToolResult::failure("Escritura cancelada por el usuario."));
        }
    }
    if ctx.ui.dry_run {
        return Ok(ToolResult {
            content: format!(
                "DRY-RUN: se escribirian {} bytes en {}",
                content.len(),
                path.display()
            ),
            success: true,
            mutated: false,
            verified: false,
            finish: None,
        });
    }
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            bail!(
                "no se escribe sobre un enlace simbolico: {}",
                path.display()
            );
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let backup = backup_file(&path, &ctx.config.permissions.backup_dir)?;
    write_atomic(&path, content.as_bytes(), 0o644)?;
    let mut message = format!("Escrito {} ({} bytes).", path.display(), content.len());
    if let Some(backup) = backup {
        message.push_str(&format!(" Backup: {}", backup.display()));
    }
    Ok(ToolResult::mutation(message, false))
}

fn tool_ask_user(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let question = arg_str(args, "question")?;
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("text");
    match kind {
        "secret" => {
            let value = ctx.ui.ask_secret(&question)?;
            let reference = ctx.secrets.insert(value);
            Ok(ToolResult::ok(format!(
                "Secreto guardado como {reference}. Usa la referencia {{{{ {reference} }}}} dentro de un comando; no escribas el valor en claro."
            )))
        }
        "choice" => {
            let options = args
                .get("options")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if options.is_empty() {
                let answer = ctx.ui.ask_text(&question)?;
                return Ok(ToolResult::ok(format!("Respuesta del usuario: {answer}")));
            }
            let list = options
                .iter()
                .enumerate()
                .map(|(i, o)| format!("{}. {o}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            let answer = ctx
                .ui
                .ask_text(&format!("{question}\n{list}\nElige un numero"))?;
            let index = answer
                .parse::<usize>()
                .ok()
                .filter(|i| *i >= 1 && *i <= options.len())
                .with_context(|| format!("opcion invalida: {answer}"))?;
            Ok(ToolResult::ok(format!(
                "Respuesta del usuario: {}",
                options[index - 1]
            )))
        }
        _ => {
            let answer = ctx.ui.ask_text(&question)?;
            Ok(ToolResult::ok(format!("Respuesta del usuario: {answer}")))
        }
    }
}

fn tool_collect_logs(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let lines = args
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or(80)
        .clamp(1, 500);
    let unit = args.get("unit").and_then(Value::as_str).map(str::trim);
    let command = if let Some(unit) = unit.filter(|u| !u.is_empty()) {
        if !unit
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.@:-".contains(c))
        {
            bail!("nombre de unidad invalido");
        }
        format!(
            "journalctl -u {} -n {} --no-pager",
            shell_quote(unit),
            lines
        )
    } else {
        format!("journalctl -n {lines} --no-pager ; echo '--- dmesg ---' ; dmesg | tail -n {lines}")
    };
    let output = run_readonly(&command, ctx.config.agent.max_tool_output_bytes)?;
    let secret_values = ctx.secrets.values();
    Ok(ToolResult::ok(redact(
        &format!("$ {command}\n{}", output.combined()),
        &secret_values,
    )))
}

fn tool_install_packages(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let packages = string_array(args, "packages")?;
    let reason = arg_str(args, "reason")?;
    if packages.is_empty() {
        bail!("no se indico ningun paquete");
    }
    match ctx.config.permissions.install {
        InstallPolicy::Never => {
            return Ok(ToolResult::failure(
                "La instalacion de paquetes esta deshabilitada en la configuracion.",
            ))
        }
        InstallPolicy::Ask => {
            if !ctx.ui.confirm(
                &format!(
                    "Instalar paquetes: {}\nMotivo: {}\n¿Continuar?",
                    packages.join(", "),
                    reason
                ),
                false,
            ) {
                return Ok(ToolResult::failure("Instalacion cancelada por el usuario."));
            }
        }
        InstallPolicy::Allow => {}
    }
    if ctx.ui.dry_run {
        return Ok(ToolResult {
            content: format!(
                "DRY-RUN: se instalarian los paquetes: {}",
                packages.join(", ")
            ),
            success: true,
            mutated: false,
            verified: false,
            finish: None,
        });
    }
    let report = packages::install_packages(&packages)?;
    Ok(ToolResult::mutation(report.message, report.verified))
}

fn tool_network_status(ctx: &mut ToolContext<'_>) -> Result<ToolResult> {
    let report = network::status_report(
        ctx.platform.network_backend,
        ctx.config.agent.max_tool_output_bytes,
    );
    let secret_values = ctx.secrets.values();
    Ok(ToolResult::ok(redact(
        &format!("{}\n\n{}", ctx.platform.summary(), report),
        &secret_values,
    )))
}

fn tool_wifi_connect(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let ssid = arg_str(args, "ssid")?;
    if ssid.trim().is_empty() {
        bail!("el SSID no puede estar vacio");
    }
    let hidden = args.get("hidden").and_then(Value::as_bool).unwrap_or(false);
    let interface = args.get("interface").and_then(Value::as_str);
    let mut password: Option<String> = None;
    if let Some(reference) = args.get("password_ref").and_then(Value::as_str) {
        let reference = reference
            .trim()
            .trim_start_matches("{{")
            .trim_end_matches("}}")
            .trim();
        let value = ctx
            .secrets
            .get(reference)
            .with_context(|| format!("referencia secreta no encontrada: {reference}"))?
            .to_string();
        password = Some(value);
    }
    if password.is_none() {
        if let Some(value) = args.get("password").and_then(Value::as_str) {
            if !value.is_empty() {
                password = Some(value.to_string());
            }
        }
    }
    let iface_for_risk = interface
        .map(str::to_string)
        .or_else(crate::platform::board::default_route_interface);
    if let Some(iface) = &iface_for_risk {
        if ssh_risk_for(iface)
            && !ctx.ui.danger_confirm(&format!(
                "La conexion WiFi en {iface} puede cortar la sesion SSH actual. ¿Continuar?"
            ))
        {
            return Ok(ToolResult::failure("Cambio de WiFi cancelado."));
        }
    }
    if ctx.config.permissions.mode == PermissionMode::ConfirmAll
        && !ctx.ui.confirm(
            &format!(
                "Conectar a la red WiFi '{}' en {}?",
                ssid,
                iface_for_risk.as_deref().unwrap_or("interfaz auto")
            ),
            false,
        )
    {
        return Ok(ToolResult::failure("Cambio de WiFi cancelado."));
    }
    if ctx.ui.dry_run {
        return Ok(ToolResult {
            content: format!(
                "DRY-RUN: se conectaria a '{}' en {} con {} (oculta={})",
                ssid,
                iface_for_risk.as_deref().unwrap_or("interfaz auto"),
                ctx.platform.network_backend.label(),
                hidden
            ),
            success: true,
            mutated: false,
            verified: false,
            finish: None,
        });
    }
    let report = network::wifi_connect(
        ctx.platform.network_backend,
        interface,
        &ssid,
        password.as_deref(),
        hidden,
        &ctx.config.permissions.backup_dir,
    )?;
    Ok(ToolResult::mutation(report.message, report.verified))
}

fn tool_set_static_ip(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let interface = arg_str(args, "interface")?;
    let ip_cidr = arg_str(args, "ip_cidr")?;
    let gateway = arg_str(args, "gateway")?;
    let dns = optional_string_array(args, "dns");
    if ssh_risk_for(&interface) {
        if !command_exists("systemd-run") {
            return Ok(ToolResult::failure(
                "No se puede cambiar la IP por SSH sin systemd-run para programar el rollback automatico.",
            ));
        }
        if !ctx.ui.danger_confirm(&format!(
            "Cambiar la IP de {interface} puede cortar tu sesion SSH.\nSe programara un rollback automatico a los 180 segundos si no se verifica.\n¿Continuar?"
        )) {
            return Ok(ToolResult::failure("Cambio de IP cancelado."));
        }
    } else if ctx.config.permissions.mode == PermissionMode::ConfirmAll
        && !ctx.ui.confirm(
            &format!("Configurar IP fija {ip_cidr} en {interface} (gateway {gateway})?"),
            false,
        )
    {
        return Ok(ToolResult::failure("Cambio de IP cancelado."));
    }
    if ctx.ui.dry_run {
        return Ok(ToolResult {
            content: format!(
                "DRY-RUN: se configuraria {interface} con {ip_cidr}, gateway {gateway}, DNS {} mediante {}",
                if dns.is_empty() { "sin cambios".to_string() } else { dns.join(", ") },
                ctx.platform.network_backend.label()
            ),
            success: true,
            mutated: false,
            verified: false,
            finish: None,
        });
    }
    let report = network::set_static_ip(
        ctx.platform.network_backend,
        &interface,
        &ip_cidr,
        &gateway,
        &dns,
        &ctx.config.permissions.backup_dir,
    )?;
    Ok(ToolResult::mutation(report.message, report.verified))
}

fn tool_load_skill(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    let name = arg_str(args, "name")?;
    let skill = ctx
        .skills
        .find(&name)
        .with_context(|| format!("skill no encontrada: {name}"))?;
    if !ctx.loaded_skills.iter().any(|s| s == &name) {
        if ctx.loaded_skills.len() >= ctx.config.skills.max_loaded {
            bail!(
                "limite de {} skills cargadas alcanzado",
                ctx.config.skills.max_loaded
            );
        }
        ctx.loaded_skills.push(name.clone());
    }
    let secret_values = ctx.secrets.values();
    let mut content = format!(
        "# Skill: {}\n{}\nRuta: {}",
        skill.name,
        skill.description,
        skill.path.display()
    );
    if let Some(compat) = &skill.compatibility {
        content.push_str(&format!("\nCompatibilidad: {compat}"));
    }
    if let Some(boards) = skill.metadata.get("simplebot/boards") {
        content.push_str(&format!("\nPlacas: {boards}"));
        let current = board_slug(ctx.platform.board.kind);
        if !boards
            .split(',')
            .map(str::trim)
            .any(|board| board == current)
        {
            content.push_str(&format!(
                "\nAVISO: esta skill no declara compatibilidad con la placa detectada ({current}); revisa sus instrucciones antes de aplicarla."
            ));
        }
    }
    content.push_str("\n\n---\n\n");
    content.push_str(&skill.body);
    Ok(ToolResult::ok(redact(&content, &secret_values)))
}

fn tool_save_skill(ctx: &mut ToolContext<'_>, args: &Value) -> Result<ToolResult> {
    if ctx.config.skills.generation == GenerationMode::Off {
        return Ok(ToolResult::failure(
            "La generacion de skills esta deshabilitada en la configuracion.",
        ));
    }
    let name = arg_str(args, "name")?;
    let description = arg_str(args, "description")?;
    let instructions = arg_str(args, "instructions")?;
    let compatibility = args
        .get("compatibility")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| Some(ctx.platform.board.summary()));
    let overwrite = args
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let secret_values = ctx.secrets.values();
    let generated_text = format!("{description}\n{instructions}");
    if secret_values
        .iter()
        .any(|secret| secret.len() >= 3 && generated_text.contains(secret))
    {
        return Ok(ToolResult::failure(
            "La skill contiene un secreto conocido. Usa ask_user(kind=secret) en las instrucciones y no guardes el valor.",
        ));
    }
    let mut metadata = BTreeMap::new();
    metadata.insert(
        "simplebot/boards".to_string(),
        board_slug(ctx.platform.board.kind),
    );
    let root = skill_root(ctx.config);
    let preview = skills::generate::render_skill_md(
        &name,
        &description,
        &instructions,
        compatibility.as_deref(),
        &metadata,
    )?;
    let preview = redact(&preview, &secret_values);
    if ctx.ui.dry_run {
        return Ok(ToolResult {
            content: format!(
                "DRY-RUN: se guardaria la skill en {}\n\n{}",
                root.join(&name).join("SKILL.md").display(),
                truncate_middle(&preview, 4000)
            ),
            success: true,
            mutated: false,
            verified: false,
            finish: None,
        });
    }
    if !ctx.ui.confirm(
        &format!(
            "Guardar skill '{}' en {}?\n\n{}",
            name,
            root.join(&name).join("SKILL.md").display(),
            truncate_middle(&preview, 6000)
        ),
        false,
    ) {
        return Ok(ToolResult::failure("Creacion de skill cancelada."));
    }
    let path = skills::generate::save_skill(
        &root,
        &name,
        &description,
        &instructions,
        compatibility.as_deref(),
        &metadata,
        overwrite,
        &ctx.config.permissions.backup_dir,
    )?;
    Ok(ToolResult::mutation(
        format!("Skill guardada en {}", path.display()),
        false,
    ))
}

fn tool_finish(args: &Value) -> Result<ToolResult> {
    let success = args
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let summary = arg_str(args, "summary")?;
    let evidence = optional_string_array(args, "evidence");
    Ok(ToolResult::finish(FinishCall {
        success,
        summary,
        evidence,
    }))
}

fn authorize(ctx: &mut ToolContext<'_>, risk: Risk, command: &str, purpose: &str) -> Result<bool> {
    if ctx.ui.dry_run {
        return Ok(true);
    }
    match risk {
        Risk::Safe => Ok(true),
        Risk::Low => {
            if ctx.ui.show_commands {
                ctx.ui.risk(risk.label(), command);
            }
            Ok(true)
        }
        Risk::Medium => {
            if ctx.config.permissions.mode == PermissionMode::ReadOnly {
                return Ok(false);
            }
            ctx.ui.risk(risk.label(), command);
            Ok(ctx.ui.confirm(
                &format!(
                    "Ejecutar comando (riesgo {}) para: {purpose}?",
                    risk.label()
                ),
                false,
            ))
        }
        Risk::High => {
            ctx.ui.risk(risk.label(), command);
            if ctx.config.permissions.mode == PermissionMode::ReadOnly {
                return Ok(false);
            }
            Ok(ctx.ui.confirm(
                &format!("Comando de riesgo ALTO para: {purpose}.\n¿Ejecutar de todos modos?"),
                false,
            ))
        }
        Risk::Blocked => {
            if !ctx.config.permissions.allow_dangerous {
                ctx.ui.error("comando bloqueado por seguridad");
                return Ok(false);
            }
            ctx.ui.risk(risk.label(), command);
            Ok(ctx.ui.danger_confirm(&format!(
                "Comando potencialmente destructivo para: {purpose}.\n{command}"
            )))
        }
    }
}

fn skill_root(config: &Config) -> PathBuf {
    let preferred = if is_root() {
        PathBuf::from("/etc/simplebot/skills")
    } else {
        crate::config::home_dir().join(".config/simplebot/skills")
    };
    let paths: Vec<PathBuf> = config
        .skills
        .paths
        .iter()
        .map(|path| crate::config::expand_path(path))
        .collect();
    if paths.iter().any(|path| path == &preferred) {
        return preferred;
    }
    if is_root() {
        if let Some(path) = paths.into_iter().find(|path| path.is_absolute()) {
            return path;
        }
    } else {
        let home = crate::config::home_dir();
        if let Some(path) = paths
            .into_iter()
            .find(|path| path.is_absolute() && path.starts_with(&home))
        {
            return path;
        }
    }
    preferred
}

fn board_slug(kind: crate::platform::board::BoardKind) -> String {
    match kind {
        crate::platform::board::BoardKind::RaspberryPi => "raspberry-pi".to_string(),
        crate::platform::board::BoardKind::OrangePi4Pro => "orange-pi-4-pro".to_string(),
        crate::platform::board::BoardKind::GenericArm => "generic-arm64".to_string(),
        crate::platform::board::BoardKind::Other => "other".to_string(),
    }
}

fn effective_write_path(path: &Path) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    match fs::canonicalize(parent) {
        Ok(parent) => parent.join(path.file_name().unwrap_or_default()),
        Err(_) => path.to_path_buf(),
    }
}

fn expand_path(value: &str) -> PathBuf {
    let path = crate::config::expand_path(Path::new(value));
    let path = if path.is_absolute() {
        path
    } else {
        Path::new("/").join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
        }
    }
    normalized
}

fn arg_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("falta el argumento '{key}'"))
}

fn string_array(args: &Value, key: &str) -> Result<Vec<String>> {
    let Some(items) = args.get(key).and_then(Value::as_array) else {
        bail!("falta el array '{key}'");
    };
    Ok(items
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect())
}

fn optional_string_array(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_helpers() {
        assert!(ToolResult::ok("x").success);
        assert!(!ToolResult::failure("x").success);
        assert!(ToolResult::mutation("x", true).verified);
    }

    #[test]
    fn read_only_blocks_writes() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.permissions.mode = PermissionMode::ReadOnly;
        config.permissions.audit_log = dir.path().join("audit.jsonl");
        config.agent.session_log = false;
        let platform = PlatformInfo::detect(&config.platform);
        let skills = SkillRegistry::default();
        let audit = AuditLogger::new(config.permissions.audit_log.clone(), Vec::new());
        let mut ui = Ui::test();
        let mut secrets = SecretStore::new();
        let mut loaded = Vec::new();
        let mut ctx = ToolContext {
            config: &config,
            platform: &platform,
            skills: &skills,
            ui: &mut ui,
            secrets: &mut secrets,
            loaded_skills: &mut loaded,
            audit: &audit,
            session_id: "test",
        };
        let target = dir.path().join("no-crear.txt");
        let result = execute_tool(
            &mut ctx,
            "write_file",
            json!({"path": target, "content": "x", "reason": "test"}),
        );
        assert!(!result.success);
        assert!(!target.exists());
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(
            expand_path("/etc/../etc/passwd"),
            PathBuf::from("/etc/passwd")
        );
        assert_eq!(expand_path("etc/hostname"), PathBuf::from("/etc/hostname"));
    }

    #[test]
    fn argument_helpers() {
        let args = json!({"a": "b", "list": ["x", "y"]});
        assert_eq!(arg_str(&args, "a").unwrap(), "b");
        assert_eq!(string_array(&args, "list").unwrap(), vec!["x", "y"]);
        assert!(arg_str(&args, "missing").is_err());
    }
}
