#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -d "$SCRIPT_DIR/config" && -d "$SCRIPT_DIR/examples" ]]; then
  ROOT="$SCRIPT_DIR"
else
  ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
fi
if [[ -z "${1:-}" && -f "$ROOT/simplebot" ]]; then
  BIN="$ROOT/simplebot"
else
  BIN="${1:-$ROOT/target/aarch64-unknown-linux-musl/release/simplebot}"
fi

if [[ ! -f "$BIN" ]]; then
  echo "No se encuentra el binario: $BIN" >&2
  echo "Compila primero con cargo (ver README) o pasa la ruta como primer argumento." >&2
  exit 1
fi

case "$(uname -m)" in
  aarch64|arm64|x86_64|amd64) ;;
  *) echo "Aviso: arquitectura $(uname -m) no probada. Se instalara igualmente el binario indicado." >&2 ;;
esac

if [[ "${EUID}" -eq 0 ]]; then
  SUDO=""
else
  SUDO="sudo"
fi

$SUDO install -d -m 755 /usr/local/bin
$SUDO install -d -m 750 /etc/simplebot /etc/simplebot/skills
$SUDO install -d -m 750 /var/log/simplebot /var/backups/simplebot
$SUDO install -m 755 "$BIN" /usr/local/bin/simplebot

if [[ ! -f /etc/simplebot/config.toml ]]; then
  $SUDO install -m 640 "$ROOT/config/config.toml.example" /etc/simplebot/config.toml
  echo "Configuracion instalada en /etc/simplebot/config.toml"
else
  echo "Se conserva /etc/simplebot/config.toml existente"
fi

if [[ -d "$ROOT/examples/skills" ]]; then
  for skill in "$ROOT"/examples/skills/*; do
    [[ -d "$skill" ]] || continue
    name="$(basename "$skill")"
    if [[ ! -e "/etc/simplebot/skills/$name" ]]; then
      $SUDO cp -r "$skill" "/etc/simplebot/skills/$name"
      $SUDO chmod -R u=rwX,go=rX "/etc/simplebot/skills/$name"
    fi
  done
fi

cat <<'EOF'

Instalado.

Siguientes pasos:
  1. Guarda la API key:
       sudo install -m 600 /dev/null /etc/simplebot/api_key
       sudo nano /etc/simplebot/api_key
     O define OPENCODE_API_KEY.
  2. Comprueba:
       sudo simplebot doctor
  3. Uso:
       sudo simplebot
       sudo simplebot "configura el wifi"

La configuracion esta en /etc/simplebot/config.toml.
Las skills del sistema, en /etc/simplebot/skills.
La auditoria, en /var/log/simplebot/audit.jsonl.
Los backups, en /var/backups/simplebot.
EOF
