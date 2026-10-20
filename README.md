# simplebot

Agente de administracion en Rust para Raspberry Pi y Orange Pi. Le escribes una tarea en lenguaje natural, inspecciona el sistema, ejecuta cambios con permisos, comprueba el resultado y corrige leyendo logs.

Pensado para consumir pocos recursos: I/O bloqueante, sin runtime asincrono, un solo binario ARM64 y REPL por SSH.

## Funciones

- Tareas de administracion: red, WiFi, IP fija, servicios, ficheros, usuarios, logs, paquetes y comandos generales.
- Verificacion obligatoria despues de cada cambio antes de declarar exito.
- Bucle de correccion: si falla, recoge logs y cambia de estrategia.
- Root con permisos por riesgo, backups y auditoria.
- WiFi e IP fija con NetworkManager, netplan o systemd-networkd/wpa_supplicant.
- Rollback automatico en cambios de red que puedan cortar SSH.
- Instalacion de paquetes con simulacion y confirmacion.
- Skills compatibles con `SKILL.md`, con generacion bajo demanda y confirmacion.
- Un unico binario para Raspberry Pi OS 64-bit y Orange Pi 4 Pro (A733).

## Requisitos

- Linux ARM64 (Raspberry Pi OS 64-bit, Orange Pi OS Debian/Ubuntu, Armbian equivalente).
- Root para ejecutar tareas.
- `bash`, `systemd`, `iproute2`.
- `NetworkManager` (`nmcli`) o `netplan`, segun la imagen.
- `apt` para instalar paquetes.
- API key de OpenCode Go o de otro proveedor compatible con OpenAI Chat Completions.

## Descargar y ejecutar (sin compilar)

Cada tag `v*` publica una release en GitHub con:

- `simplebot-linux-arm64.tar.gz` — Raspberry Pi OS 64-bit y Orange Pi 4 Pro.
- `simplebot-linux-amd64.tar.gz` — PC/Ubuntu x86_64.
- Binarios sueltos, `SHA256SUMS`, `config.toml.example` e `install.sh`.

En la placa, comprueba la arquitectura:

```bash
uname -m
# aarch64 -> usa arm64
# x86_64  -> usa amd64
```

Descarga la release:

```bash
cd /tmp
wget https://github.com/manologcode/simplebot/releases/latest/download/simplebot-linux-arm64.tar.gz
wget https://github.com/manologcode/simplebot/releases/latest/download/SHA256SUMS

sha256sum -c SHA256SUMS --ignore-missing
tar -xzf simplebot-linux-arm64.tar.gz
cd simplebot-linux-arm64

sudo ./install.sh ./simplebot
```

El `install.sh` instala `/usr/local/bin/simplebot`, crea `/etc/simplebot`, `/var/log/simplebot` y `/var/backups/simplebot`, copia la configuracion de ejemplo y las skills de ejemplo. Si ya existe `/etc/simplebot/config.toml`, no lo sobrescribe.

Configura la API key:

```bash
sudo install -m 600 /dev/null /etc/simplebot/api_key
sudo nano /etc/simplebot/api_key
```

Comprueba y arranca:

```bash
sudo simplebot doctor
sudo simplebot
```

Tambien puedes usar `gh`:

```bash
gh release download --repo manologcode/simplebot \
  --pattern 'simplebot-linux-arm64.tar.gz' \
  --pattern SHA256SUMS
```

Si no hay release del tag, cada ejecucion del workflow deja los binarios descargables en `Actions -> build-release -> simplebot-binaries`.

### Solo el binario

Si prefieres no usar el tar.gz:

```bash
wget https://github.com/manologcode/simplebot/releases/latest/download/simplebot-linux-arm64
chmod +x simplebot-linux-arm64
sudo install -m 755 simplebot-linux-arm64 /usr/local/bin/simplebot
sudo install -d -m 750 /etc/simplebot /etc/simplebot/skills
sudo install -d -m 750 /var/log/simplebot /var/backups/simplebot
wget https://github.com/manologcode/simplebot/releases/latest/download/config.toml.example
sudo install -m 640 config.toml.example /etc/simplebot/config.toml
```

### Imagen Docker

La imagen multi-arquitectura se publica en GitHub Container Registry:

```bash
# Si el paquete es privado:
# echo "$GITHUB_TOKEN" | docker login ghcr.io -u manologcode --password-stdin

docker pull ghcr.io/manologcode/simplebot:latest

# Prueba sin acceso al host
docker run --rm -it ghcr.io/manologcode/simplebot:latest doctor

# Ejecucion con acceso al host (revisa antes docker-compose.yml)
docker run --rm -it \
  --privileged \
  --network host \
  --pid host \
  -v /etc/simplebot:/etc/simplebot \
  -v /var/log/simplebot:/var/log/simplebot \
  -v /var/backups/simplebot:/var/backups/simplebot \
  ghcr.io/manologcode/simplebot:latest
```

`docker run` sin configuracion usa los valores por defecto; para el modo host se recomienda usar el `docker-compose.yml` incluido. Recuerda que `apt-get` dentro del contenedor instala en el contenedor, no en el host.

## Compilar

En la propia placa:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
sudo apt install build-essential pkg-config
cargo build --release
sudo install -m 755 target/release/simplebot /usr/local/bin/simplebot
```

Cross-compile recomendado (binario estatico, compatible con ambas placas):

```bash
rustup target add aarch64-unknown-linux-musl
cargo install cargo-zigbuild
# Requiere Zig instalado: https://ziglang.org/download/
cargo zigbuild --release --target aarch64-unknown-linux-musl
```

Probado con Zig 0.17.0 y `cargo-zigbuild`; el binario resultante es estatico, ARM64 y de unos 3,3 MB. Asegurate de que `zig` este en `PATH` (`cargo zigbuild` tambien acepta `--zig-path`).

Alternativa con `cross` y Docker:

```bash
cargo install cross
cross build --release --target aarch64-unknown-linux-musl
```

Alternativa glibc, si el enlazado estatico falla:

```bash
rustup target add aarch64-unknown-linux-gnu
sudo apt install gcc-aarch64-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --target aarch64-unknown-linux-gnu
```


## Docker y GitHub Actions

El repositorio incluye un `Dockerfile` multi-stage, `docker-compose.yml` y el workflow `.github/workflows/release.yml`.

Construir y ejecutar toda la validacion dentro de Docker:

```bash
docker buildx build --target test .
```

Generar los binarios estaticos (amd64 y arm64) sin instalar Rust en el host:

```bash
docker buildx build \
  --target artifacts \
  --platform linux/amd64 \
  --output type=local,dest=dist \
  .
# Resultado:
# dist/out/simplebot-linux-amd64
# dist/out/simplebot-linux-arm64
```

Construir y publicar la imagen multi-arquitectura:

```bash
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  --target runtime \
  -t ghcr.io/manologcode/simplebot:latest \
  --push .
```

Nota: `linux/arm6` no es un platform valido de Docker. Para Raspberry Pi y Orange Pi de 64 bits se usa `linux/arm64`; para 32 bits serian `linux/arm/v6` o `linux/arm/v7`, que este workflow no publica.

Para construir `linux/arm64` en un host amd64 hace falta emulacion (QEMU/binfmt). El workflow ya configura `docker/setup-qemu-action`; en local puedes usar `docker run --privileged --rm tonistiigi/binfmt --install arm64`.

Ejecucion con acceso al host (solo en una placa dedicada):

```bash
docker compose run --rm simplebot
```

El compose usa `privileged`, `pid: host`, `network_mode: host` y monta `/etc/NetworkManager`, `/etc/netplan`, `/etc/systemd/network`, `/etc/wpa_supplicant`, D-Bus y los directorios de configuracion, logs y backups. Aun asi, **`apt-get` dentro del contenedor instala en el contenedor, no en el host**, los ficheros fuera de los mounts se modifican dentro del contenedor y la deteccion de distro/placa puede reflejar el contenedor. Para administrar el host al 100% (paquetes incluidos), ejecuta el binario estatico con `sudo` en la placa.

### Publicar en GitHub

Si el repositorio aun no tiene remoto:

```bash
git init
git branch -M main
git add .
git commit -m "simplebot: agente de administracion"
git remote add origin git@github.com:manologcode/simplebot.git
git push -u origin main
```

Despues, para generar una release descargable:

```bash
git tag v0.1.0
git push origin v0.1.0
```

### Release automatica

El workflow se ejecuta al subir cambios a `main`/`master`, al crear un tag `v*` y manualmente. En cada ejecucion:

1. ejecuta `fmt`, `clippy` y los tests dentro de Docker;
2. construye y publica `ghcr.io/manologcode/simplebot` para `linux/amd64` y `linux/arm64`;
3. exporta binarios, tarballs, `SHA256SUMS`, `config.toml.example` e `install.sh`;
4. en tags, adjunta todo a la release de GitHub; en `main` queda como artifact descargable.

Tambien se puede lanzar manualmente desde `Actions -> build-release -> Run workflow`. El `GITHUB_TOKEN` del repositorio se usa para GHCR; no hace falta crear un secreto adicional. Si el paquete aparece privado, cambia su visibilidad en GitHub Packages.

Si usas `docker compose`, ejecuta antes `deploy/install.sh` o crea `/etc/simplebot` con tu configuracion y skills; el compose monta ese directorio del host sobre el contenedor.

## Instalar desde el codigo

```bash
sudo ./deploy/install.sh target/aarch64-unknown-linux-musl/release/simplebot
```

Despues:

```bash
sudo install -m 600 /dev/null /etc/simplebot/api_key
sudo nano /etc/simplebot/api_key   # pega la API key
sudo simplebot doctor
sudo simplebot
```

La API key tambien puede venir de `OPENCODE_API_KEY` o de `provider.api_key` en la configuracion. Para `sudo`, usa `sudo -E` o el fichero de clave; es preferible el fichero 0600.

## Configuracion

Se busca en este orden:

1. `--config <ruta>`
2. `SIMPLEBOT_CONFIG`
3. `/etc/simplebot/config.toml`
4. `~/.config/simplebot/config.toml`
5. `./simplebot.toml`

Plantilla completa: `config/config.toml.example`.

```toml
[provider]
base_url = "https://opencode.ai/inference/go/openai/v1"
model = "deepseek-v4.1-flash"
api_key_env = "OPENCODE_API_KEY"
api_key_file = "/etc/simplebot/api_key"

[permissions]
mode = "risky"       # read_only | risky | confirm_all
install = "ask"      # ask | never | allow
allow_dangerous = false

[skills]
generation = "confirm"
```

OpenCode Go expone modelos compatibles con OpenAI en `https://opencode.ai/inference/go/openai/v1`. El cliente usa el endpoint `chat/completions`, envia un `User-Agent` propio y `x-opencode-session`, como recomienda la documentacion de Go. Si un modelo no soporta tool calls nativos, se acepta como respaldo una respuesta JSON con la forma `{"tool":"nombre","arguments":{...}}`. Los modelos de Go que usan `/responses` o `/messages` no son compatibles con este cliente; elige uno de `chat/completions` (por ejemplo `deepseek-v4.1-flash`, `glm-5.3-flash` o `kimi-k2.7-code`). Tambien puedes apuntar a Ollama, LM Studio, OpenRouter u otro endpoint compatible cambiando `base_url`, `model` y la clave.

## Uso

```bash
sudo simplebot
simplebot> configura el wifi
simplebot> pon una IP fija en eth0
simplebot> instala htop y comprueba que funciona
simplebot> revisa por que no hay internet
simplebot> crea una skill para diagnosticar la red

sudo simplebot "configura el wifi"
sudo simplebot --dry-run "cambia la IP de eth0"
sudo simplebot doctor
```

En el REPL:

- `/help`
- `/doctor`
- `/skills`
- `/config`
- `/clear`
- `/exit`

## Permisos

Con `mode = "risky"`:

| Tipo | Comportamiento |
|---|---|
| Lectura (`ip`, `journalctl`, `cat` de rutas no sensibles) | Automatico |
| Cambios conocidos con backup/rollback (WiFi, IP fija) | Automatico con verificacion |
| Comandos desconocidos o de riesgo medio | Pide confirmacion |
| Instalar/desinstalar, borrar, reiniciar, ficheros criticos | Pide confirmacion |
| Comandos destructivos (`rm -rf /`, `mkfs`, `dd` a disco) | Bloqueados; solo con `allow_dangerous` y confirmacion escrita `SI` |

Las contrasenas se piden con `ask_user(kind=secret)` y se guardan en memoria como `secretref-N`. El modelo ve la referencia, no el valor. Los secretos se redactan en auditoria, sesiones y salida de herramientas.

Rutas sensibles como `/etc/shadow`, claves SSH, `/etc/NetworkManager/system-connections` y `/etc/wpa_supplicant` no se leen directamente y requieren confirmacion si un comando las toca.

## Skills

Cada skill es una carpeta con un `SKILL.md`:

```
/etc/simplebot/skills/configurar-wifi/SKILL.md
```

```markdown
---
name: configurar-wifi
description: Conecta la placa a una red WiFi y verifica la conectividad.
compatibility: Raspberry Pi OS, Orange Pi OS
metadata:
  simplebot/boards: "raspberry-pi,orange-pi-4-pro"
---

## Procedimiento
...
```

El agente solo ve nombre y descripcion hasta que llama a `skill(name)`. Las skills son instrucciones: no pueden saltarse permisos. La generacion se hace bajo demanda:

```bash
sudo simplebot skills new "diagnostica la red"
sudo simplebot skills list
sudo simplebot skills show configurar-wifi
sudo simplebot skills run configurar-wifi "conecta a la red casa"
sudo simplebot skills validate
sudo simplebot skills remove configurar-wifi
```

Al guardar una skill se muestra el contenido y se pide confirmacion. No se guardan secretos.

Ejemplos listos para copiar en `examples/skills/`:

- `configurar-wifi`
- `configurar-ip-fija`
- `diagnosticar-red`

## Red

Backends soportados:

- NetworkManager: Raspberry Pi OS y Orange Pi OS cuando esta activo.
- netplan: imagenes Ubuntu/Orange Pi OS con renderer networkd.
- systemd-networkd + wpa_supplicant: fallback.

`simplebot doctor` muestra el backend detectado. En WiFi, la contrasena se escribe en un fichero de perfil 0600, no en la linea de comandos. En IP fija se guarda un backup y se programa un rollback con `systemd-run` a los 180 segundos; si la verificacion es correcta, se cancela.

Con netplan, la IP fija sobre WiFi no se modifica de forma automatica para no romper el bloque `access-points`; usa NetworkManager para ese caso. La deteccion del renderer evita mezclar netplan con NetworkManager.

## Seguridad

Un agente con root puede hacer mucho dano. Este proyecto reduce el riesgo, pero no lo elimina:

- Revisa las confirmaciones y no uses `--yes` a ciegas.
- No uses `allow_dangerous` salvo en un entorno controlado.
- Manten la API key en un fichero 0600.
- Revisa las skills generadas antes de usarlas.
- Los logs y salidas de comandos pueden contener datos sensibles; se redactan patrones conocidos, pero no es infalible.
- OpenCode Go esta orientado a agentes de codigo. Si el trafico de administracion resulta atipico, usa otro proveedor compatible.

## Desarrollo

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo build --release
```

Pruebas cubiertas: clasificador de riesgos, redaccion de secretos, configuracion, skills, timeouts, parser del proveedor y bucle del agente con verificacion obligatoria.

## Limitaciones

- El modelo deberia soportar tool calling nativo; si no, funciona el respaldo JSON, pero es menos fiable.
- Los cambios de red pueden requerir adaptar el backend a una imagen concreta; `doctor` y `network_status` ayudan a diagnosticarlo.
- No hay control por voz; las ordenes se escriben por teclado.
- No hay TUI; el REPL es deliberadamente simple.
