# 007 — Descoberta, nomes e roteamento

## Regra inviolável: nunca escrever IP

Em 03/11 o Windows saiu de `192.168.1.43` para `192.168.1.42` (mesmo
MAC `00:00:5e:00:53:01`). Quebrou de uma vez: Deskflow (`No route to
host` em loop), SSH, e o painel. **O GSConnect sobreviveu** — descobre
por broadcast.

A lição: o que descobre por nome se cura; o que é fixado em IP quebra.

## mDNS

```
/etc/nsswitch.conf:  hosts: files mdns4_minimal [NOTFOUND=return] dns
avahi-daemon:        active
libnss-mdns:         0.15.1-5
```

`getent hosts windows-host.local` → `192.168.1.42`, estável. Todas as
configs usam o nome: `Deskflow.conf`, `~/.ssh/config` e
`~/.config/logi/config.toml`.

Quirks pagos:

- **Deskflow não aceita lista com vírgula** no `remoteHost`, apesar do
  tooltip da GUI prometer. `FATAL: No server addresses configured`.
  Um host só.
- **`ssh-keyscan <nome>.local` falha** com `Temporary failure in name
  resolution` mesmo com `getent` funcionando — `mdns4_minimal` +
  `[NOTFOUND=return]` em consulta AF_UNSPEC. O `ssh` em si resolve.
- **`known_hosts` é por nome.** Trocar IP→nome dá `Host key
  verification failed`. A correção certa é copiar a *mesma* chave
  confiável para o novo nome e conferir fingerprint — nunca
  `accept-new` às cegas.

## O que o `logid` deve fazer

1. **Resolver sempre na hora.** Nunca cachear IP em config; cachear no
   máximo em runtime, com TTL curto.
2. **Avisar quando o IP mudar.** Mesmo funcionando por nome, registrar
   o evento: IP dançando é sintoma de falta de reserva de DHCP.
3. **Checar pré-requisitos no startup** e expor na API: `avahi-daemon`
   ativo, `mdns4_minimal` no nsswitch, ACL do `hidraw`.

## Pendência fora do nosso alcance

Reserva de DHCP no roteador para o MAC `00:00:5e:00:53:01`. mDNS
depende de avahi e multicast; a reserva é a redundância. Não dá para
automatizar — é ação no roteador.

## Identidade de dispositivo (Logitech)

Ver [`005`](005-input-switch.md). Mapeamento nesta máquina:

| hidraw | dispositivo | slot no receptor |
|---|---|---|
| `hidraw0` | `046D:C52B` receptor Unifying | — |
| `hidraw2` | `046D:4090` MX Keys | 2 |
| `hidraw3` | `046D:408A` MX Anywhere 3 | 1 |

Os dois estão pareados aqui **e** no receptor do Windows, em canais
Easy-Switch diferentes. É isso que torna a rota física possível.
