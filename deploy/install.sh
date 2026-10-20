#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$ROOT/target/aarch64-unknown-linux-musl/release/simplebot}"

if [[ ! -f "$BIN" ]]; then
  echo "No se encuentra el binario: $BIN" >&2
  echo "Compila primero con cargo (ver README) o pasa la ruta como primer argumento." >&2
  exit 1
fi

if [[ "$(uname -m)" != "aarch64" ]]; then
  echo "Aviso: esta maquina es $(uname -m), no aarch64. Se instalara igualmente el binario indicado." >&2
fi

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
