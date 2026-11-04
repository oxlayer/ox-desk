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

---

## Implementado (03/11) — nativo de cada SO, com a GUI despachando

**Decisão do usuário**, posta contra a alternativa de um RustDesk
auto-hospedado. O RustDesk tinha dois argumentos reais a favor — um
cliente igual nos três SOs, e só conexão de **saída**, que importaria num
Mac com MDM — e perdeu porque é uma dependência externa grande e um relay
a manter, enquanto o nativo não sobe daemon novo em lugar nenhum.

Cada máquina já sabe ser controlada, por um protocolo diferente. Não
existe um que os três sirvam de fábrica:

| host | protocolo | servidor | estado |
|---|---|---|---|
| Ubuntu | RDP | `gnome-remote-desktop` (daemon de **usuário**) | já ativo, `*:3389`, TLS e credenciais postos |
| Windows | RDP | `TermService` nativo | ligado em 03/11, NLA obrigatório, firewall só na LAN |
| macOS | VNC | Compartilhamento de Tela | a habilitar pelo usuário |

### [`logid/src/screen.rs`](../logid/src/screen.rs) não implementa protocolo

Ele responde duas perguntas — *a tela daquele host está alcançável?* e
*existe cliente aqui para falar com ela?* — e, a pedido, lança o cliente.
Vídeo, input e TLS são dos programas que já existem.

Por isso o `GET /api/screen` é separado do `POST`: a UI precisa saber se o
botão vai funcionar **antes** de a pessoa clicar. Um botão que abre nada é
pior que um botão desabilitado com o motivo escrito do lado — e o motivo
é específico (*"sem cliente rdp instalado aqui (procurei: …)"*,
*"3389 não respondeu"*), não um "indisponível".

A tabela de clientes por plataforma vive em `sys::screen_clients`, em
ordem de preferência. No Linux o `gnome-connections` vem primeiro porque
fala **RDP e VNC**: um app cobre o Windows e o Mac, em vez de dois
clientes com duas UIs.

> `{u_flag}` no template existe porque o freerdp quer `/u:nome` como **um**
> argumento, e quando não há usuário o argumento tem de *desaparecer* —
> passar `/u:` vazio faz ele pedir usuário em prompt e travar sem terminal.
> Travado por `template_de_usuario_some_quando_nao_ha_usuario`.

### O Windows foi ligado com escopo, não aberto

`fDenyTSConnections = 0` e as regras de firewall do modo de usuário
habilitadas — mas com `-RemoteAddress 192.168.1.0/24` e só nos perfis
`Private,Domain`. A regra vem de fábrica com `Any` nos dois, o que abriria
RDP em **qualquer rede** a que o notebook se conectasse. `UserAuthentication = 1`
(NLA) para autenticar antes de pintar tela de login. A regra de *sombra*
(assistir sessão de outro) fica desligada.

### Verificado

- **Windows → Ubuntu: pronto.** `mstsc.exe /v:ubuntu-host.local:3389`,
  alvo respondendo em 0,4 ms. Não disparei: abrir RDP contra o
  `gnome-remote-desktop` de usuário espelha a sessão que a pessoa está
  usando, e esse clique é dela.
- **Ubuntu → Windows: 3389 já abre** daqui; falta só o cliente, que entra
  pelo [`scripts/enable-sshd.sh`](../scripts/enable-sshd.sh) junto do
  `openssh-server`.
- **Mac**: nos três hosts da config com `expect = "any"`, acende sozinho
  quando o Compartilhamento de Tela for habilitado.

### Falta

- Cliente VNC no Windows (nada vem de fábrica) para alcançar o Mac.
- Cliente RDP no Mac — `brew install --cask windows-app`.
- Atalho na bandeja do Electron, hoje só no painel.
