---
name: diagnosticar-red
description: Revisa interfaces, rutas, DNS, WiFi y logs para diagnosticar problemas de conectividad sin cambiar la configuracion.
license: MIT
compatibility: Raspberry Pi OS 64-bit, Orange Pi OS Debian/Ubuntu
metadata:
  simplebot/boards: "raspberry-pi,orange-pi-4-pro"
---

## Cuando usar

Cuando el usuario diga que no hay Internet, que el WiFi no conecta o que la red va lenta.

## Procedimiento

1. Ejecuta `network_status`.
2. Comprueba interfaz y direccion IPv4 con `ip -4 -brief address`.
3. Comprueba ruta por defecto con `ip route`.
4. Comprueba DNS con `resolvectl status` y `getent hosts opencode.ai`.
5. Comprueba gateway con `ping -c 1 -W 3 <gateway>`.
6. Revisa logs con `collect_logs` para NetworkManager, wpa_supplicant o systemd-networkd segun el backend.
7. Si el adaptador no aparece, revisa `dmesg | tail -n 80` y `lsusb`.
8. No cambies nada hasta explicar el diagnostico. Si propones un cambio, pide confirmacion.

## Verificacion

- Resumen claro de que capa falla: enlace, IP, ruta, DNS o aplicacion.
- Evidencia de cada comprobacion.
