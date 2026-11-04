# 000 — Visão geral

> Escrito para: você e os agentes que forem trabalhar neste repo.

## A lacuna que justifica o projeto

Da sua lista — SSH, file browser, KDE Connect, trocar de PC por borda ou
atalho, clipboard, compartilhar tela — **quase tudo já existe e funciona
bem em open source**. Reimplementar seria gastar meses para chegar pior.

Uma coisa não existe. É esta:

> Decidir **como** trocar de computador: mover o cursor pela LAN, ou
> mandar o próprio mouse/teclado pular de canal Easy-Switch para o
> receptor físico da outra máquina.

Isso é exatamente o que o **Logitech Flow** faz, é o que você queria no
começo, e é a única peça que nenhum projeto aberto resolve:

- O **Solaar** sabe trocar o canal do dispositivo (`change-host`, HID++
  0x1814) mas não sabe nada de rede.
- O **Deskflow** sabe mover o cursor pela rede mas não toca no HID++.
- O **GSConnect / KDE Connect** sabem falar com a outra máquina mas não
  fazem nem um nem outro.

Ninguém cola as três coisas. **Esse é o produto.** O resto é a casca
que torna isso utilizável e que reaproveita o que já existe.

## O que construímos e o que integramos

O critério: construímos **orquestração, estado e UI**. Integramos
**transporte, codec e protocolo** — camadas onde um projeto maduro já
resolveu problemas que levariam anos.

| Capacidade | Decisão | Por quê |
|---|---|---|
| Roteamento de input (LAN ↔ receptor físico) | **construir** | não existe; é o núcleo do projeto |
| Control plane + API + estado | **construir** | é a cola; ninguém faz |
| GUI de configuração (Electron) | **construir** | já temos a casca e o painel |
| Orquestração de SSH / bootstrap das máquinas | **construir** | específico do nosso par de máquinas |
| File browser | **construir** (fino) | é UI sobre SFTP, que o OpenSSH já entrega |
| Clipboard | **construir** (fino) | é relay de texto; o difícil é política, não protocolo |
| Mover cursor entre telas | **integrar** Deskflow / lan-mouse | protocolo + injeção de input por portal; maduro |
| Compartilhar tela / controle remoto | **integrar** RustDesk (self-hosted) ou Sunshine+Moonlight | captura PipeWire/DXGI, encode H.264/AV1, NAT, latência. Escopo de produto. |
| Descoberta e pareamento TLS na LAN | **integrar** o modelo do KDE Connect | protocolo testado, e já estamos pareados |

Reimplementar um RustDesk significaria: captura de tela (portal
ScreenCast no Wayland, DXGI no Windows), encode com hardware, transporte
com tuning de latência, travessia de NAT, injeção de input, cripto e
auth. Não é projeto de fim de semana, e o RustDesk é **self-hostável** —
`hbbs`/`hbbr` próprios, ou conexão direta em LAN, sem servidor de
terceiro. "Próprio e seguro" sem escrever nada.

## Arquitetura

Simétrica: **todo nó roda o mesmo daemon e nenhum é central.** Quem
manda é a máquina onde a pessoa está. Ver
[`001`](001-control-plane.md#simétrico-todo-nó-é-par-nenhum-é-central).

```
   ubuntu-host            windows-host              MacBook (canal 3)
  ┌──────────────┐        ┌──────────────┐        ┌──────────────┐
  │ GUI          │        │ GUI          │        │ GUI          │
  │  ↕           │        │  ↕           │        │  ↕           │
  │ logid        │◀─SSH──▶│ logid        │◀─SSH──▶│ logid        │
  │  ├ input     │        │  ├ input     │        │  ├ input     │
  │  ├ hidpp     │        │  ├ hidpp     │        │  ├ hidpp     │
  │  └ adapters  │        │  └ adapters  │        │  └ adapters  │
  └──────┬───────┘        └──────┬───────┘        └──────┬───────┘
         │ canal 1               │ canal 2               │ canal 3
         └───────────── MX Anywhere 3 + MX Keys ──────────┘
                    (exatamente um detentor por vez)
```

Pedir foco é encaminhamento: quem pede fala com o **detentor atual**,
porque só ele alcança o dispositivo por rádio.

## Linguagem: Rust

O control plane é **Rust** ([`logid/`](../logid/)), decisão tomada em
03/11 e já implementada na fase 1.

O protótipo era Python stdlib, e funcionava. A migração aconteceu antes
de a complexidade crescer — que é o momento barato de fazer isso, não
depois. O que se ganhou, medido:

| | Python | Rust |
|---|---|---|
| `/healthz` | 0,6 ms | **0,19 ms** |
| binário | — | 929 KB, sem runtime |
| config inválida | erro em runtime, na requisição | **recusada no boot**, com exit 1 |
| campo desconhecido no PATCH | precisaria de validação manual | recusado pelo tipo, 400 |

O ganho real não é latência (I/O domina: `/api/status` leva 2 s nas
duas versões, por causa do SSH). É o **tipo como contrato**: `Expect`
é enum, não string; `Config` não desserializa se o TOML estiver errado;
e o compilador recusa o caso não tratado. Num daemon que vai disparar
troca de canal HID++ e pode te deixar sem teclado
([`005`](005-input-switch.md#rollback-não-é-opcional)), isso vale mais
que microssegundos.

Dependências, deliberadamente poucas: `tiny_http` (bloqueante — dois
clientes não justificam async), `serde`, `serde_json`, `toml`. Sem
crate de data: `date +%z` resolve o offset local.

O **agente Windows** também será Rust, pelo mesmo motivo pelo qual o
HID++ 0x1814 vai ser nosso: binário único, sem runtime para instalar
do outro lado.

## Fases

| Fase | Entrega | Estado |
|---|---|---|
| **0** | Setup manual funcionando, documentado | ✅ feito — ver [README](../README.md) |
| **—** | Deskflow e Solaar **removidos** dos dois lados | ✅ a troca é física; nenhum dos dois é mais necessário |
| **1** | `logid` servindo status + config editável | ✅ feito — Rust, `PATCH /api/config` validado |
| **2** | SSH + bootstrap automático das duas máquinas | [`002`](002-ssh.md) |
| **3** | Roteamento de input: LAN × receptor físico | 🚧 `GET /api/devices` e `POST /api/focus` (dry-run) prontos; falta detecção de borda e o agente Windows |
| **4** | Clipboard e file browser | [`003`](003-files.md), [`004`](004-clipboard.md) |
| **5** | Compartilhar tela (integração) | [`006`](006-screen-share.md) |

Fase 3 antes de 4 e 5 de propósito: é a única que ninguém mais faz.

## Não-objetivos

- Internet, nuvem, conta, telemetria. Só LAN.
- Suportar hardware que não seja o seu. Generalizar depois, se doer.
- Reimplementar codec de vídeo, protocolo de KVM ou stack de descoberta.
- Mobile. O GSConnect já resolve celular.

## Ler também

- [`001-control-plane.md`](001-control-plane.md) — o daemon e a API
- [`005-input-switch.md`](005-input-switch.md) — o núcleo do projeto
- [`008-security.md`](008-security.md) — identidade e trust model
- [`../README.md`](../README.md) — o que já está rodando hoje
