use anyhow::{bail, Result};
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(Debug, Clone)]
pub struct Ui {
    pub color: bool,
    pub show_commands: bool,
    pub auto_yes: bool,
    pub dry_run: bool,
    pub interactive: bool,
}

impl Ui {
    pub fn new(color: bool, show_commands: bool, auto_yes: bool, dry_run: bool) -> Self {
        Self {
            color,
            show_commands,
            auto_yes,
            dry_run,
            interactive: io::stdin().is_terminal(),
        }
    }

    pub fn test() -> Self {
        Self {
            color: false,
            show_commands: false,
            auto_yes: true,
            dry_run: false,
            interactive: false,
        }
    }

    pub fn banner(&self) {
        println!(
            "{}",
            self.bold("simplebot 0.1 — agente de administracion para Raspberry Pi / Orange Pi")
        );
        if self.dry_run {
            println!(
                "{}",
                self.yellow("MODO DRY-RUN: no se ejecutaran cambios reales")
            );
        }
    }

    pub fn info(&self, text: &str) {
        println!("{}", self.cyan(text));
    }

    pub fn success(&self, text: &str) {
        println!("{}", self.green(text));
    }

    pub fn warn(&self, text: &str) {
        eprintln!("{}", self.yellow(text));
    }

    pub fn error(&self, text: &str) {
        eprintln!("{}", self.red(text));
    }

    pub fn tool_call(&self, name: &str, arguments: &str) {
        if self.show_commands {
            println!(
                "{} {} {}",
                self.dim("herramienta:"),
                self.bold(name),
                self.dim(arguments)
            );
        }
    }

    pub fn risk(&self, risk: &str, command: &str) {
        println!(
            "{} {} {}",
            self.yellow("riesgo:"),
            self.bold(risk),
            self.dim(command)
        );
    }

    pub fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        if self.auto_yes {
            self.info(&format!("{question} [auto: si]"));
            return true;
        }
        if !self.interactive {
            self.warn(&format!("{question} [sin terminal interactiva: no]"));
            return false;
        }
        let hint = if default_yes { "[S/n/A]" } else { "[s/N/A]" };
        loop {
            print!("{question} {hint} ");
            let _ = io::stdout().flush();
            let mut line = String::new();
            if io::stdin().lock().read_line(&mut line).is_err() {
                return false;
            }
            let answer = line.trim().to_lowercase();
            if answer.is_empty() {
                return default_yes;
            }
            if ["s", "si", "sí", "y", "yes"].contains(&answer.as_str()) {
                return true;
            }
            if ["n", "no"].contains(&answer.as_str()) {
                return false;
            }
            if ["a", "auto", "todo"].contains(&answer.as_str()) {
                self.auto_yes = true;
                self.info("auto: no se volvera a preguntar en esta sesion");
                return true;
            }
        }
    }

    pub fn danger_confirm(&mut self, question: &str) -> bool {
        if self.auto_yes {
            self.warn(&format!("{question} [auto: SI]"));
            return true;
        }
        if !self.interactive {
            self.warn(&format!("{question} [sin terminal interactiva: no]"));
            return false;
        }
        println!("{}", self.red(question));
        print!("Escribe SI en mayusculas para continuar: ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).is_err() {
            return false;
        }
        line.trim() == "SI"
    }

    pub fn ask_text(&mut self, question: &str) -> Result<String> {
        if !self.interactive {
            bail!("se necesitan datos del usuario pero no hay terminal interactiva");
        }
        print!("{question}: ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Ok(line.trim().to_string())
    }

    pub fn ask_secret(&mut self, question: &str) -> Result<String> {
        if !self.interactive {
            bail!("se necesita un secreto pero no hay terminal interactiva");
        }
        let value = rpassword::prompt_password(format!("{question}: "))?;
        if value.is_empty() {
            bail!("el valor introducido esta vacio");
        }
        Ok(value)
    }

    pub fn result_preview(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let preview = crate::process::truncate_middle(text, 2000);
        println!("{}", self.dim(&preview));
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }

    fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }

    fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }

    fn cyan(&self, text: &str) -> String {
        self.paint("36", text)
    }
}
