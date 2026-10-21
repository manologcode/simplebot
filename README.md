# simplebot

Agente de administracion para Raspberry Pi y Orange Pi. Le escribes una tarea en lenguaje natural y la ejecuta como root: inspecciona el sistema, hace cambios, revisa los logs, verifica el resultado y corrige si algo falla.

Funciona en Raspberry Pi OS 64-bit, Orange Pi OS Debian/Ubuntu y Linux ARM64/x86_64. Pide confirmacion antes de operaciones peligrosas, guarda backups y registra todo en una auditoria.

## Descargar e instalar

No hace falta clonar el repositorio. Descarga el paquete desde la ultima release:

**Raspberry Pi / Orange Pi (ARM64):**

```bash
cd /tmp
curl -fLO https://github.com/manologcode/simplebot/releases/latest/download/simplebot-linux-arm64.tar.gz
curl -fLO https://github.com/manologcode/simplebot/releases/latest/download/SHA256SUMS

sha256sum -c SHA256SUMS --ignore-missing
tar -xzf simplebot-linux-arm64.tar.gz
cd simplebot-linux-arm64

sudo ./install.sh ./simplebot
```

**PC / Ubuntu (x86_64):**

```bash
cd /tmp
curl -fLO https://github.com/manologcode/simplebot/releases/latest/download/simplebot-linux-amd64.tar.gz
curl -fLO https://github.com/manologcode/simplebot/releases/latest/download/SHA256SUMS

sha256sum -c SHA256SUMS --ignore-missing
tar -xzf simplebot-linux-amd64.tar.gz
cd simplebot-linux-amd64

sudo ./install.sh ./simplebot
```

El instalador deja el programa en `/usr/local/bin/simplebot`, crea los directorios de configuracion, logs y backups, e instala la configuracion y las skills de ejemplo.

Release: <https://github.com/manologcode/simplebot/releases/latest>

## Configurar la API key

```bash
sudo install -m 600 /dev/null /etc/simplebot/api_key
sudo nano /etc/simplebot/api_key
```

Pega dentro la API key de OpenCode Go. Tambien puedes usar la variable `OPENCODE_API_KEY` o editar `/etc/simplebot/config.toml`.

La configuracion por defecto usa OpenCode Go y el modelo `deepseek-v4.1-flash`. Para cambiar de modelo o de proveedor edita:

```bash
sudo nano /etc/simplebot/config.toml
```

## Actualizar

Comprueba si hay una nueva version:

```bash
simplebot update --check
```

Descarga y sustituye el binario por la ultima version (verifica el SHA256):

```bash
sudo simplebot update
```

El comando conserva la configuracion, las skills, la auditoria y los backups.

## Uso

Comprueba la instalacion:

```bash
sudo simplebot doctor
```

Abre el modo interactivo:

```bash
sudo simplebot
```

Ejemplos de tareas:

```text
simplebot> configura el wifi
simplebot> pon una IP fija en eth0
simplebot> instala htop y comprueba que funciona
simplebot> revisa por que no hay internet
simplebot> crea una skill para diagnosticar la red
```

Tambien puedes ejecutar una tarea suelta:

```bash
sudo simplebot "configura el wifi"
sudo simplebot "pon una IP fija en eth0"
sudo simplebot --dry-run "cambia la IP de eth0"
```

Comandos del modo interactivo:

- `/help` — ayuda
- `/doctor` — comprobar plataforma, configuracion y conectividad
- `/skills` — listar skills
- `/config` — mostrar configuracion
- `/exit` — salir

## Permisos

- En las confirmaciones puedes responder `A` para aprobar automaticamente el resto de la sesion.
- Las lecturas y los cambios conocidos con verificacion se ejecutan directamente.
- Los comandos desconocidos, instalaciones, borrados y cambios criticos piden confirmacion.
- Los comandos destructivos estan bloqueados salvo que se active `allow_dangerous`.
- Usa `--dry-run` para ver que haria sin cambiar nada.

Las contrasenas se piden de forma oculta y no se guardan en los logs.

## Skills

```bash
sudo simplebot skills list
sudo simplebot skills show configurar-wifi
sudo simplebot skills run configurar-wifi "conecta a la red casa"
sudo simplebot skills new "diagnostica la red"
```

Las skills son instrucciones reutilizables guardadas en `/etc/simplebot/skills`. Al crear una se muestra el contenido y se pide confirmacion.

## Docker

```bash
# Si el paquete es privado: docker login ghcr.io -u manologcode
docker pull ghcr.io/manologcode/simplebot:latest
docker run --rm -it ghcr.io/manologcode/simplebot:latest doctor
```

Para trabajar sobre el host, usa el `docker-compose.yml` incluido:

```bash
docker compose run --rm simplebot
```

El contenedor necesita modo privilegiado y acceso a la red y al PID del host. Ten en cuenta que `apt-get` dentro del contenedor instala en el contenedor, no en el host; para administrar el host al 100% usa el binario con `sudo`.
