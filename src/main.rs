use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use simplebot::agent::Agent;
use simplebot::audit::AuditLogger;
use simplebot::config::{Config, GenerationMode};
use simplebot::platform::PlatformInfo;
use simplebot::process::{backup_file, command_exists, is_root, timestamp};
use simplebot::provider::openai::OpenAiProvider;
use simplebot::safety::redact::redact;
use simplebot::skills::SkillRegistry;
use simplebot::ui::Ui;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(
    name = "simplebot",
    version,
    about = "Agente de administracion para Raspberry Pi y Orange Pi"
)]
struct Cli {
    /// Ruta al fichero de configuracion TOML
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Simula los cambios sin ejecutarlos
    #[arg(long, global = true)]
    dry_run: bool,

    /// Aprueba automaticamente las operaciones no bloqueadas
    #[arg(long, global = true)]
    yes: bool,

    /// Permite comandos peligrosos con confirmacion escrita
    #[arg(long, global = true)]
    allow_dangerous: bool,

    /// Log detallado por stderr
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,

    /// Tarea en lenguaje natural
    #[arg(value_name = "TAREA")]
    task: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Comprueba configuracion, plataforma y conectividad
    Doctor,
    /// Comprueba si hay una nueva version y actualiza la aplicacion
    Update {
        /// Solo comprueba si hay una nueva version
        #[arg(long)]
        check: bool,
        /// Reinstala la ultima version aunque ya este actualizada
        #[arg(long)]
        force: bool,
    },
    /// Gestiona las skills
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },
}

#[derive(Subcommand, Debug)]
enum SkillsAction {
    /// Lista las skills disponibles
    List,
    /// Muestra una skill completa
    Show { name: String },
    /// Valida todas las skills
    Validate,
    /// Genera una skill nueva con el agente
    New { description: String },
    /// Ejecuta una skill, opcionalmente con una tarea
    Run {
        name: String,
        #[arg(default_value = "")]
        task: String,
    },
    /// Elimina una skill
    Remove { name: String },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn init_logging(verbose: bool) {
    let default = if verbose { "debug" } else { "warn" };
    let env = env_logger::Env::default().default_filter_or(default);
    env_logger::Builder::from_env(env).init();
}

fn run(cli: Cli) -> Result<std::process::ExitCode> {
    let (mut config, config_path) = Config::load(cli.config.as_deref())?;
    if cli.allow_dangerous {
        config.permissions.allow_dangerous = true;
    }

    match cli.command {
        Some(Command::Doctor) => {
            doctor(&config, config_path.as_deref());
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(Command::Update { check, force }) => {
            simplebot::update::run(check, force)?;
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(Command::Skills { action }) => {
            return skills_command(action, &config);
        }
        None => {}
    }

    if !is_root() && !cli.dry_run {
        bail!("simplebot necesita root para administrar el sistema. Ejecuta: sudo simplebot");
    }

    if let Some(task) = cli.task {
        return run_one_task(&config, &task, cli.dry_run, cli.yes);
    }
    run_repl(&config, cli.dry_run, cli.yes)
}

fn run_one_task(
    config: &Config,
    task: &str,
    dry_run: bool,
    yes: bool,
) -> Result<std::process::ExitCode> {
    let _lock = SingleInstance::acquire()?;
    let session_id = Uuid::new_v4().to_string();
    let mut ui = Ui::new(
        config.ui.color,
        config.ui.show_commands,
        yes || dry_run,
        dry_run,
    );
    ui.banner();
    let audit = AuditLogger::new(config.permissions.audit_log.clone(), Vec::new());
    let platform = PlatformInfo::detect(&config.platform);
    let skills = SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
    for warning in &skills.warnings {
        ui.warn(&format!("skill: {warning}"));
    }
    let provider = build_provider(config, &session_id)?;
    let mut agent = Agent {
        config,
        provider: &provider,
        platform,
        skills,
        ui: &mut ui,
        audit: &audit,
        session_id,
    };
    let outcome = agent.run_task(task)?;
    if let Some(path) = outcome.session_path {
        ui.info(&format!("Sesion: {}", path.display()));
    }
    Ok(if outcome.success {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    })
}

fn run_repl(config: &Config, dry_run: bool, yes: bool) -> Result<std::process::ExitCode> {
    let _lock = SingleInstance::acquire()?;
    let session_id = Uuid::new_v4().to_string();
    let mut ui = Ui::new(
        config.ui.color,
        config.ui.show_commands,
        yes || dry_run,
        dry_run,
    );
    ui.banner();
    ui.info("Escribe una tarea o /help. Ctrl-D para salir.");
    let audit = AuditLogger::new(config.permissions.audit_log.clone(), Vec::new());
    let platform = PlatformInfo::detect(&config.platform);
    let mut provider = match build_provider(config, &session_id) {
        Ok(provider) => Some(provider),
        Err(err) => {
            ui.warn(&format!(
                "No hay proveedor de IA disponible todavia: {err:#}. Puedes usar /doctor, /skills y /config."
            ));
            None
        }
    };

    let mut editor = rustyline::DefaultEditor::new()?;
    loop {
        match editor.readline("simplebot> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);
                match line {
                    "/exit" | "/quit" | "exit" | "quit" => break,
                    "/help" => print_help(),
                    "/doctor" => doctor(config, None),
                    "/skills" => {
                        let registry =
                            SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
                        print_skills(&registry);
                    }
                    "/config" => {
                        println!("{}", redact(&config.to_toml(), &[]));
                    }
                    "/clear" => {
                        print!("\x1b[2J\x1b[H");
                        let _ = std::io::stdout().flush();
                    }
                    _ => {
                        if provider.is_none() {
                            match build_provider(config, &session_id) {
                                Ok(new_provider) => provider = Some(new_provider),
                                Err(err) => {
                                    ui.error(&format!("{err:#}"));
                                    continue;
                                }
                            }
                        }
                        let skills =
                            SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
                        for warning in &skills.warnings {
                            ui.warn(&format!("skill: {warning}"));
                        }
                        let mut agent = Agent {
                            config,
                            provider: provider.as_ref().expect("proveedor comprobado"),
                            platform: platform.clone(),
                            skills,
                            ui: &mut ui,
                            audit: &audit,
                            session_id: session_id.clone(),
                        };
                        match agent.run_task(line) {
                            Ok(outcome) => {
                                if let Some(path) = outcome.session_path {
                                    ui.info(&format!("Sesion: {}", path.display()));
                                }
                            }
                            Err(err) => ui.error(&format!("{err:#}")),
                        }
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(err) => {
                ui.error(&format!("entrada: {err}"));
                break;
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn build_provider(config: &Config, session_id: &str) -> Result<OpenAiProvider> {
    let api_key = config.api_key()?;
    Ok(OpenAiProvider::new(
        &config.provider,
        api_key,
        session_id.to_string(),
    ))
}

fn skills_command(action: SkillsAction, config: &Config) -> Result<std::process::ExitCode> {
    match action {
        SkillsAction::List => {
            let registry = SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
            print_skills(&registry);
            for warning in &registry.warnings {
                eprintln!("aviso: {warning}");
            }
            Ok(std::process::ExitCode::SUCCESS)
        }
        SkillsAction::Show { name } => {
            let registry = SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
            let skill = registry
                .find(&name)
                .with_context(|| format!("skill no encontrada: {name}"))?;
            println!("nombre: {}", skill.name);
            println!("descripcion: {}", skill.description);
            println!("ruta: {}", skill.path.display());
            if let Some(compat) = &skill.compatibility {
                println!("compatibilidad: {compat}");
            }
            for (key, value) in &skill.metadata {
                println!("{key}: {value}");
            }
            println!("\n---\n\n{}", skill.body);
            Ok(std::process::ExitCode::SUCCESS)
        }
        SkillsAction::Validate => {
            let registry = SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
            println!("{} skills validas", registry.skills.len());
            for warning in &registry.warnings {
                eprintln!("error: {warning}");
            }
            Ok(if registry.warnings.is_empty() {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            })
        }
        SkillsAction::New { description } => {
            if config.skills.generation == GenerationMode::Off {
                bail!("la generacion de skills esta deshabilitada");
            }
            if !is_root() {
                bail!("generar skills del sistema requiere root. Ejecuta: sudo simplebot skills new ...");
            }
            let task = format!(
                "Crea una skill reutilizable para esta tarea: {description}\n\nInspecciona la plataforma actual, redacta instrucciones claras con pasos, comandos y verificacion, y guardala con save_skill. No incluyas secretos. Usa un nombre en minusculas con guiones."
            );
            run_one_task(config, &task, false, false)
        }
        SkillsAction::Run { name, task } => {
            if !is_root() {
                bail!("ejecutar una skill puede cambiar el sistema; usa sudo");
            }
            let registry = SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
            let skill = registry
                .find(&name)
                .with_context(|| format!("skill no encontrada: {name}"))?;
            let prompt = if task.trim().is_empty() {
                format!(
                    "Aplica la skill '{}' llamando primero a skill(name). Sigue su procedimiento y pide los datos que falten. Si la skill no encaja con ninguna tarea concreta, explica como usarla y espera instrucciones.",
                    skill.name
                )
            } else {
                format!(
                    "Aplica la skill '{}' llamando primero a skill(name) y sigue su procedimiento para esta tarea: {}",
                    skill.name, task
                )
            };
            run_one_task(config, &prompt, false, false)
        }
        SkillsAction::Remove { name } => {
            if !is_root() {
                bail!("eliminar skills del sistema requiere root");
            }
            let registry = SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
            let skill = registry
                .find(&name)
                .with_context(|| format!("skill no encontrada: {name}"))?;
            let mut ui = Ui::new(config.ui.color, config.ui.show_commands, false, false);
            if !ui.confirm(
                &format!(
                    "Eliminar la skill '{}' de {}?",
                    skill.name,
                    skill.path.display()
                ),
                false,
            ) {
                return Ok(std::process::ExitCode::FAILURE);
            }
            backup_file(&skill.path, &config.permissions.backup_dir)?;
            fs::remove_file(&skill.path)?;
            if let Some(parent) = skill.path.parent() {
                if parent
                    .read_dir()
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(false)
                {
                    let _ = fs::remove_dir(parent);
                }
            }
            println!("skill '{}' eliminada", skill.name);
            Ok(std::process::ExitCode::SUCCESS)
        }
    }
}

fn doctor(config: &Config, config_path: Option<&Path>) {
    println!("simplebot doctor");
    println!("================");
    println!(
        "root: {}",
        if is_root() {
            "si"
        } else {
            "NO (se requiere sudo para tareas)"
        }
    );
    println!(
        "config: {}",
        config_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "valores por defecto (no hay fichero)".to_string())
    );
    let platform = PlatformInfo::detect(&config.platform);
    println!("plataforma: {}", platform.summary());
    println!(
        "proveedor: {} | modelo: {} | tool calling: native + fallback JSON",
        config.provider.base_url, config.provider.model
    );
    println!(
        "permisos: modo={:?} install={:?} dangerous={}",
        config.permissions.mode, config.permissions.install, config.permissions.allow_dangerous
    );
    println!(
        "rutas: auditoria={} backups={}",
        config.permissions.audit_log.display(),
        config.permissions.backup_dir.display()
    );

    println!("\nherramientas:");
    for tool in [
        "nmcli",
        "netplan",
        "networkctl",
        "journalctl",
        "systemctl",
        "apt-get",
        "systemd-run",
        "ip",
        "ping",
    ] {
        println!(
            "  {tool}: {}",
            if command_exists(tool) { "si" } else { "no" }
        );
    }

    let registry = SkillRegistry::discover(&config.skills.paths, config.skills.max_bytes);
    println!("\nskills: {} validas", registry.skills.len());
    for skill in &registry.skills {
        println!("  {}: {}", skill.name, skill.description);
    }
    for warning in &registry.warnings {
        eprintln!("  aviso: {warning}");
    }

    println!("\nAPI key:");
    match config.api_key() {
        Ok(_) => {
            println!("  presente");
            match build_provider(config, "doctor") {
                Ok(provider) => match provider.check_connection() {
                    Ok(message) => println!("  {message}"),
                    Err(err) => println!("  aviso: {err:#}"),
                },
                Err(err) => println!("  error: {err:#}"),
            }
        }
        Err(err) => println!("  no disponible: {err}"),
    }

    if let Some(path) = config_path {
        if config.provider.api_key.is_some() {
            if let Ok(meta) = fs::metadata(path) {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o077 != 0 {
                    eprintln!(
                        "  aviso: {} contiene una API key y es legible por otros usuarios",
                        path.display()
                    );
                }
            }
        }
    }
    if let Some(file) = &config.provider.api_key_file {
        if let Ok(meta) = fs::metadata(file) {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                eprintln!(
                    "  aviso: {} es legible por otros usuarios; usa chmod 600",
                    file.display()
                );
            }
        }
    }

    println!("\nshell: /bin/bash | fecha: {}", timestamp());
}

fn print_skills(registry: &SkillRegistry) {
    if registry.skills.is_empty() {
        println!("no hay skills en las rutas configuradas");
        return;
    }
    for skill in &registry.skills {
        println!("{:<32} {}", skill.name, skill.description);
    }
}

fn print_help() {
    println!(
        "Comandos:\n  <tarea>        ejecutar una tarea en lenguaje natural\n  /doctor        comprobar plataforma y configuracion\n  /skills        listar skills\n  /config        mostrar configuracion efectiva\n  /clear         limpiar pantalla\n  /exit          salir\n\nConfirmaciones: s = si, n = no, A = auto (no vuelve a preguntar en la sesion)\n\nEjemplos:\n  configura el wifi\n  pon una IP fija en eth0\n  instala htop\n  crea una skill para revisar el estado de la red"
    );
}

struct SingleInstance {
    path: PathBuf,
}

impl SingleInstance {
    fn acquire() -> Result<Self> {
        let path = if is_root() {
            PathBuf::from("/run/simplebot.lock")
        } else {
            std::env::temp_dir().join("simplebot.lock")
        };
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
                Ok(Self { path })
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let pid = fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| text.trim().parse::<u32>().ok());
                if let Some(pid) = pid {
                    if Path::new(&format!("/proc/{pid}")).exists() {
                        bail!("ya hay otra instancia de simplebot en ejecucion (pid {pid})");
                    }
                }
                let _ = fs::remove_file(&path);
                Self::acquire()
            }
            Err(err) => Err(err).context("no se pudo crear el lock de instancia"),
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
