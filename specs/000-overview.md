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

```
        ┌─────────────────────────── ubuntu-host ───────────────────────────┐
        │                                                                     │
        │   Electron (GUI)  ──HTTP──▶  logid (control plane, :8787)           │
        │                                   │                                 │
        │                     ┌─────────────┼──────────────┐                  │
        │                     ▼             ▼              ▼                  │
        │               input-router    adapters       store (JSON/sqlite)    │
        │                     │         ├─ ssh                                │
        │                     │         ├─ sftp (file browser)                │
        │                     │         ├─ clipboard                          │
        │                     │         ├─ deskflow  (ctl)                    │
        │                     │         ├─ rustdesk  (ctl)                    │
        │                     │         └─ hidpp/solaar (Easy-Switch)         │
        └─────────────────────┼───────────────────────────────────────────────┘
                              │  LAN (mDNS + TLS mútuo)
        ┌─────────────────────▼──────────── windows-host ───────────────────────┐
        │   logid-agent  (mesmo binário, modo agente)                         │
        └─────────────────────────────────────────────────────────────────────┘
```

Um binário só, dois modos. O control plane é quem decide; o agente
executa no outro lado.

## Linguagem

**Python para o control plane.** Já temos [`monitor/monitor.py`](../monitor/monitor.py)
funcionando com stdlib puro, sem pip — e control plane é I/O, não CPU.
Reescrever em Go ou Rust agora seria trocar código que funciona por
código novo sem ganho mensurável.

**Rust entra quando houver motivo medido**, e só no pedaço que o pedir:

- o caminho quente de injeção de input, se latência virar problema
  (hoje o Deskflow cobre isso);
- o agente Windows, se distribuir um binário único ficar mais simples
  que carregar Python para lá.

Escolher Rust antes de ter o número é otimização prematura — e a
camada mais valiosa (a decisão LAN × físico) não é limitada por CPU.

## Fases

| Fase | Entrega | Estado |
|---|---|---|
| **0** | Setup manual funcionando, documentado | ✅ feito — ver [README](../README.md) |
| **1** | `logid` servindo status + config read-only na GUI | painel já existe; falta a parte de config |
| **2** | SSH + bootstrap automático das duas máquinas | [`002`](002-ssh.md) |
| **3** | Roteamento de input: LAN × receptor físico | [`005`](005-input-switch.md) — **o núcleo** |
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
