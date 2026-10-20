---
name: configurar-ip-fija
description: Configura una IP estatica de forma segura, conservando el acceso SSH y verificando la conectividad.
license: MIT
compatibility: Raspberry Pi OS 64-bit, Orange Pi OS Debian/Ubuntu
metadata:
  simplebot/boards: "raspberry-pi,orange-pi-4-pro"
  simplebot/verified: "2026-10-20"
---

## Cuando usar

Cuando el usuario pida una IP fija para Ethernet o WiFi, o cambiar direccion, gateway o DNS.

## Procedimiento

1. Ejecuta `network_status` y localiza la interfaz, la conexion activa, la IP actual, el gateway y el DNS.
2. Propón los valores actuales como defecto y pide los que falten:
   - IP con prefijo CIDR, por ejemplo `192.168.1.50/24`.
   - Gateway, por ejemplo `192.168.1.1`.
   - DNS, por ejemplo `1.1.1.1` y `9.9.9.9`.
3. Comprueba si la sesion SSH usa esa misma interfaz. Si es asi, advierte del riesgo y exige confirmacion.
4. Llama a `set_static_ip`. La herramienta hace backup y programa rollback automatico a los 180 segundos si la verificacion falla.
5. Verifica direccion, ruta, gateway, ping externo y DNS.
6. Si falla, no insistas con variaciones aleatorias: restaura el perfil anterior y revisa los logs.
7. Llama a `finish` con la evidencia y la ruta del backup.

## Seguridad

- No cambies la IP de la interfaz por la que va SSH sin avisar.
- No desactives DHCP en todas las interfaces a la vez.
- No borres perfiles de red existentes.
- Mantén un backup y una forma de restaurar el acceso.

## Verificacion

- `ip -4 addr` muestra la IP esperada.
- `ip route` muestra el gateway esperado.
- El gateway y una direccion externa responden a ping.
- `getent hosts` resuelve nombres.
