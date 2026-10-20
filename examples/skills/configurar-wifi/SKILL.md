---
name: configurar-wifi
description: Conecta la placa a una red WiFi, pide las credenciales si faltan y verifica IP, ruta, gateway y DNS.
license: MIT
compatibility: Raspberry Pi OS 64-bit, Orange Pi OS Debian/Ubuntu
metadata:
  simplebot/boards: "raspberry-pi,orange-pi-4-pro"
  simplebot/verified: "2026-10-20"
---

## Cuando usar

Cuando el usuario pida conectar la placa a una red WiFi o cambiar de red inalambrica.

## Procedimiento

1. Ejecuta `network_status` para identificar el backend y la interfaz WiFi.
2. Si falta el SSID, pidelo con `ask_user` (`kind=text`).
3. Si falta la contrasena, pidela con `ask_user` (`kind=secret`). No escribas la contrasena en claro ni la guardes en la skill.
4. Si la red esta oculta, confirma el dato con el usuario.
5. Llama a `wifi_connect` con `ssid` y `password_ref`. No uses `run_command` para escribir perfiles a mano salvo para diagnosticar.
6. Si falla, recoge logs con `collect_logs` (unidad NetworkManager o wpa_supplicant) y corrige.
7. Verifica con `verify` o con la verificacion que devuelve `wifi_connect`.
8. Llama a `finish` con la evidencia: direccion IP, ruta por defecto, gateway y DNS.

## Errores habituales

- SSID con caracteres especiales: revisa que se haya pasado completo.
- Red de 5 GHz no visible: comprueba que la interfaz soporte la banda.
- Interfaz ausente: revisa `dmesg` y el driver antes de instalar firmware.
- Si la sesion SSH usa la misma interfaz, avisa antes de reconectar.

## Verificacion

- La interfaz tiene una direccion IPv4.
- Existe una ruta por defecto por esa interfaz.
- El gateway responde a ping.
- La resolucion DNS funciona (`getent hosts`).
