# 006 — Compartilhar tela

## Decisão: integrar, nunca reimplementar

Construir um RustDesk significa captura (portal ScreenCast no Wayland,
DXGI no Windows), encode H.264/AV1 com hardware, transporte com tuning
de latência, travessia de NAT, injeção de input e a camada de cripto.
Escopo de produto, com anos de trabalho — e três projetos abertos já
resolveram. O `logid` orquestra **ciclo de vida e credencial**, não
protocolo.

## O que já existe nesta LAN

### Ubuntu visto do Windows — já pronto, zero software

O `gnome-remote-desktop` 50.2 **está instalado e rodando** aqui com RDP
**habilitado** na 3389, certificado TLS próprio e credenciais
configuradas. Verificado: escutando, e o Windows alcança
(`Test-NetConnection … -Port 3389` → `True`).

Do Windows: `mstsc` → `ubuntu-host.local`. Cliente da Microsoft.

```bash
grdctl status          # estado, porta, fingerprint
grdctl rdp disable     # desligar
```

> ⚠️ É servidor RDP com senha aberto para toda a LAN, na máquina
> principal. O `logid` deve **mostrar isso como risco** no painel, não
> como feature silenciosa.

### Windows visto do Ubuntu

Windows 10 **Pro**, então tem RDP — só desligado (3389 fechada).
Ligando, `remmina` ou `freerdp3-x11` resolvem. RDP toma a sessão local:
não serve para uso simultâneo nem para jogo.

### Jogo / baixa latência

**Sunshine** (host no Windows) + **Moonlight** (cliente aqui). Encode
por hardware, latência uma ordem de magnitude abaixo de RDP ou RustDesk.

### Fora da LAN

**RustDesk self-hosted** (`hbbs`/`hbbr`). "Próprio e seguro" sem
escrever nada. Em LAN, nem precisa de relay.

## O que o `logid` faz

| Faz | Não faz |
|---|---|
| mostrar quais rotas existem e quais estão ligadas | capturar tela |
| ligar/desligar cada servidor | codificar vídeo |
| guardar credencial e lembrar fingerprint | transportar frames |
| escolher a rota pelo uso (ver abaixo) | injetar input remoto |

```
POST /api/hosts/{id}/screen   {"intent": "work" | "game" | "glance"}
```

- `work` → RDP (sessão própria, texto nítido)
- `game` → Sunshine/Moonlight (latência)
- `glance` → RustDesk (só olhar, sem tomar a sessão)

Mapear intenção para ferramenta é a parte que falta no mundo. Escolher
entre três clientes com nomes técnicos é o que torna isso irritante hoje.

## Wayland: a ressalva

Captura no Wayland passa pelo portal `ScreenCast`, mesma família de
problemas que já batemos (`InputCapture`, `libportal` 0.9.1 velho
demais para o clipboard do Deskflow). Antes de prometer qualquer rota
que capture **a tela do Ubuntu**, testar no 26.04 — não confiar em doc.
