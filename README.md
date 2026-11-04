# Logitech Flow → equivalente funcional no Ubuntu

Alvo: **Ubuntu 26.04 / GNOME 50.1 / Wayland** em `192.168.1.44`
↔ **Windows** em `192.168.1.43` (MAC `00:00:5e:00:53:01`).

## O veredito

Flow de verdade é impossível: ele exige **Options+ nas duas pontas**, e
Options+ não existe para Linux. Nenhuma alternativa de Linux (Solaar,
Piper, logiops, OpenLogi) implementa Flow — Flow é protocolo
proprietário entre as duas instâncias do Options+, não um recurso do
mouse. O OpenLogi chega a sincronizar Easy-Switch, mas não move o
ponteiro entre telas nem compartilha clipboard.

O que dá o **mesmo resultado prático** são duas peças:

| Peça do Flow | Substituto | Estado aqui |
|---|---|---|
| Ponteiro atravessa a borda da tela + teclado segue | **Deskflow** 1.27.0 (ex-Synergy CE, upstream do Barrier/Input Leap) | instalável, `.deb` já baixado |
| Copiar/colar e arrastar arquivos entre as máquinas | **GSConnect** (GNOME) + **KDE Connect** (Windows) | via apt |
| Bateria, canal Easy-Switch, DPI | **Solaar** | via apt |

(O `wl-clipboard` entrou no primeiro `install.sh` por engano meu e foi
removido: ele serviria a um workaround de clipboard do próprio Deskflow
no Wayland, que a) pede a versão `-git`, não a do apt, e b) é
desnecessário porque o clipboard aqui é do GSConnect.)

Barrier (2.4.0, 2021) e Input Leap (arquivado em jul/2026) estão
mortos — Deskflow é o mesmo código, mantido, e o único com suporte a
Wayland via libei/portals.

## Topologia: Windows é o servidor

O **servidor** é a máquina com o teclado/mouse físico; o resto é
cliente. Aqui o servidor é o **Windows**:

- É onde está o receptor Unifying que você usa, e onde o Options+
  continua te dando Smart Actions / gestos / botões.
- No Wayland o papel de **cliente** é o caminho documentado e estável
  (portal `RemoteDesktop`, exige GNOME 50+ — você tem 50.1).
- O cliente **disca para fora** na porta 24800, então o Ubuntu não
  precisa de regra de entrada.

> Detalhe que descobri sondando esta máquina: **existe um segundo
> receptor Unifying aqui** (`046d:c52b`) com o **MX Anywhere 3** e o
> **MX Keys** pareados. Ou seja, hoje você já alterna manualmente pelo
> botão Easy-Switch. Com o Deskflow você deixa os dois no canal do
> Windows e para de apertar botão.
>
> O inverso (Ubuntu como servidor) também é viável nesta máquina —
> verifiquei que o `xdg-desktop-portal-gnome` 50.0 declara **e aceita**
> `org.freedesktop.portal.InputCapture` (v1), o que a documentação do
> Deskflow ainda lista como "GNOME não implementa". É informação
> desatualizada deles, mas é caminho não testado: comece pelo que
> funciona e inverta depois se quiser.

## Ubuntu (aqui)

```bash
cd /home/user/dev/logi
sudo bash install.sh
```

Depois **saia e entre na sessão** uma vez (GSConnect é extensão do
shell) e ative-a no app **Extensões**.

## Windows side (192.168.1.43)

Copie o `windows-setup.ps1` deste diretório para o Windows e rode no
PowerShell **como administrador**:

```powershell
powershell -ExecutionPolicy Bypass -File windows-setup.ps1
```

Ele baixa e instala o Deskflow **1.27.0** (a mesma versão daqui — o
protocolo casa por versão), cria a regra de firewall na porta 24800 e
mostra se a rede está marcada como *Public* (se estiver, a regra não
vale — o script imprime o comando para trocar para *Private*).

Depois, na mão:

1. Abra o Deskflow → modo **Server**.
2. *Configure Server* → arraste uma tela nova para a grade, à esquerda
   ou direita do Windows conforme a posição física do monitor do
   Ubuntu. Nomeie a tela **exatamente** `ubuntu-host` — é o hostname
   desta máquina, e o nome é case-sensitive.
3. Instale o **KDE Connect for Windows** (Microsoft Store) para o
   clipboard/arquivos.

## Fechar o circuito

No Ubuntu, abra o Deskflow → modo **Client** → servidor
`192.168.1.43` → *Start*. Na primeira conexão as duas pontas mostram
um **fingerprint TLS** — confira que bate e aceite.

O GNOME vai pedir permissão de **controle remoto** (portal
`RemoteDesktop`): aceite e marque para lembrar, senão o diálogo
reaparece a cada sessão.

Então pareie o GSConnect com o KDE Connect do Windows (a bandeja do
GNOME lista o dispositivo) e ligue *Clipboard sync* nas duas pontas.

## Ordem importa (senão você fica sem teclado)

Hoje o MX Anywhere 3 e o MX Keys estão no canal do **receptor daqui**.
Quando você apertar o Easy-Switch para o canal do Windows, esta máquina
fica **sem teclado e sem mouse** até o Deskflow conectar. Então:

1. Windows: rode o `windows-setup.ps1`, abra o Deskflow em **Server**,
   monte a grade com a tela `ubuntu-host`.
2. Ubuntu: abra o Deskflow em **Client**, aponte para `192.168.1.43`,
   dê *Start* — ele fica tentando conectar, isso é esperado.
3. **Só então** aperte o Easy-Switch nos dois dispositivos para o canal
   do Windows.

Se algo der errado no passo 3 e você ficar sem input: aperte o
Easy-Switch de volta para o canal daqui, ou plugue um teclado USB.
Vale deixar um por perto na primeira vez.

## Layout de teclado: o `us`-only morde

Sintoma: o cliente conecta, o cursor atravessa, mas acentos, `ç` e as
teclas de ABNT2 saem errados. No log do core:

```
INFO: local layouts: en
INFO: remote layouts: pt
WARNING: server layouts missing on this computer: pt
```

O Windows está em layout português e o Ubuntu vinha só com `us`
(`localectl` mostrava `X11 Layout: us`, e `input-sources` estava
vazio). O `languageSync=true` do Deskflow tenta espelhar o layout do
servidor e não tem para onde ir.

### O detalhe que quebra a correção óbvia

Adicionar o `br` à lista de fontes **não basta**:

```bash
# isto NÃO resolve sozinho
gsettings set org.gnome.desktop.input-sources sources "[('xkb','us'),('xkb','br')]"
```

Medido: com `[us, br]` configurado e o serviço reiniciado **depois**
disso, o core ainda logou `local layouts: en` e o mesmo aviso, e
`setxkbmap -query` seguiu em `us`.

A razão é que o **GNOME não usa grupos xkb** — ele troca o keymap
inteiro conforme a fonte ativa. Então o Deskflow só enxerga **uma**
layout: a que está ativa. O `br` tem que estar *ativo*, não só na lista.

As duas saídas, dependendo do hardware:

| O teclado é… | Correção |
|---|---|
| **ABNT2** (tecla `ç` própria à direita do `L`) | torne `br` primária aqui: `sources "[('xkb','br'),('xkb','us')]"` |
| **US / internacional** | o errado é o Windows: mude **ele** para layout US |

Escolher a opção errada só troca de lado o problema.

### Neste setup: o teclado é US, então a correção é no Windows

O Ubuntu está certo (`us` ativa) e o `input-sources` voltou para `[]`,
como estava — sem seletor de layout na barra. O que sobra é alinhar o
Windows:

*Configurações → Hora e Idioma → Idioma e região →* no idioma, `…` →
*Opções de idioma → Teclados →* adicione **United States** e remova o
português (ou alterne com `Win+Espaço`).

Se você escreve em português no Windows, não use o US puro — perde os
acentos. Use **United States-International**: mesmas posições de tecla,
acentos por dead key, e é uma layout inglesa, então o Deskflow deve
reportar `en` e casar com o Ubuntu. Confirme depois com `bash status.sh`:
a seção Teclado para de avisar e o log não repete
`server layouts missing`.

Depois de mudar, **reinicie o core** (`systemctl --user restart
deskflow-client`) — ele enumera layouts só na inicialização.

## Estado real das duas máquinas (lido via SSH)

| | Ubuntu | Windows |
|---|---|---|
| hostname / tela | `ubuntu-host` | `windows-host` |
| IP | 192.168.1.44 | 192.168.1.43 |
| SO | Ubuntu 26.04, GNOME 50.1, Wayland | Windows 10 **Pro** |
| usuário | `user` | `user` |
| papel | client (`coreMode=1`) | server (`coreMode=2`) |
| autostart | `deskflow-client.service` (systemd user) | `Deskflow.lnk` em `shell:startup` |

Links da grade (`C:\ProgramData\Deskflow\deskflow-server.conf` — no
**ProgramData**, não no APPDATA, porque o `deskflow-daemon` roda como
serviço):

```
section: links
	windows-host:
		right = ubuntu-host
	ubuntu-host:
		left = windows-host
end
```

Borda **direita** do Windows → Ubuntu. Borda **esquerda** do Ubuntu →
Windows.

> No 1.27 as telas vivem na config geral (`screens\N\name=`) e o
> `deskflow-server.conf` guarda **só os links**. No Linux ele fica em
> `~/.config/Deskflow/`; no Windows, em `C:\ProgramData\Deskflow\`.

### O teclado, com o diagnóstico correto

O Windows está em **idioma pt-BR** mas com **layout de teclado
United States-International** (`0416:00020409`, decodificado no
registro). O Deskflow reporta o **idioma**, não o layout — por isso
`remote layouts: pt`. O aviso é sobre language sync, não sobre o
mapeamento físico.

A diferença que pode aparecer na prática: US-International tem **dead
keys** (`'` + `a` → `á`); o `us` puro do Ubuntu não tem. Mas o
Deskflow transmite **keysyms**, não scancodes — então é possível que o
servidor já resolva a composição e envie `á` pronto.

**Teste antes de mudar nada**: leve o cursor para a tela do Ubuntu,
abra um editor e digite `'a`, `~a`, `ç`. Se sair certo, não há o que
corrigir. Se sair `'a` literal:

```bash
# espelha o comportamento do Windows (dead keys)
gsettings set org.gnome.desktop.input-sources sources "[('xkb','us+intl')]"
systemctl --user restart deskflow-client
```

Para quem programa, `us+altgr-intl` é mais confortável (aspas literais,
acentos no AltGr) — mas aí as duas máquinas voltam a divergir.

## Diagnóstico de uma passada

```bash
bash status.sh
```

Read-only. Checa sessão, pacote, config, serviço, socket, alcance do
servidor, portais, token do Wayland, TLS/fingerprint, layout **ativa**
de teclado, limites conhecidos (libportal, Scroll Lock, bandeja) e
warnings da última hora. É o primeiro comando a rodar quando algo
parar de funcionar daqui a meses.

## Autostart e reconexão

### Ubuntu: serviço systemd de usuário

O pacote só traz `.desktop`, nenhum unit — então o repo traz o
[`deskflow-client.service`](deskflow-client.service), instalado pelo
`install.sh` em `~/.config/systemd/user/` e habilitado:

```bash
systemctl --user status deskflow-client
systemctl --user restart deskflow-client
journalctl --user -u deskflow-client -f
```

Roda o **core**, não a GUI — o ícone de bandeja não funciona no GNOME
Wayland de todo modo. É `WantedBy=graphical-session.target` porque o
cliente injeta input pelo portal/libei e precisa da sessão gráfica.

> ⚠️ Não deixe a GUI rodando junto com o serviço. O Deskflow tem guarda
> de instância única, então quem iniciar por segundo falha. Use a GUI só
> para editar settings, e feche; o serviço é o dono do processo.

### Reconexão: já é automática, não escreva poller

Medido no log com o servidor desligado:

```
connecting to '192.168.1.43': 192.168.1.43:24800
failed to connect to server: Timed out     ← timeout de 2s
retry in 1 seconds
```

Ciclo de **~3s**, indefinidamente, com `dynamicConnectionInterval=true`
adaptando. Um checador "está online?" de 60s seria **pior**: a tentativa
de TCP já *é* a detecção, e pingar antes só adiciona latência. O
`Restart=always` do unit cobre crash do processo, não servidor offline —
são coisas diferentes.

### Sem diálogo de permissão a cada boot

`~/.local/state/Deskflow.state` guarda:

```
[client]
xdpRestoreToken=<redacted>
```

É o token do portal `RemoteDesktop`. O GNOME restaura a concessão, então
a permissão de controle remoto é pedida **uma vez**, não a cada login.
Se esse arquivo for apagado, o diálogo volta.

### Windows: atalho na pasta Startup

O `windows-setup.ps1` cria `Deskflow.lnk` em `shell:startup`. É logon, não
serviço: o servidor precisa de uma sessão de usuário para capturar
teclado e mouse, então "run whether user is logged on or not" não serve.

## Ubuntu como servidor: testado e NÃO funciona (1.27.0)

Tentativa registrada para não ser repetida. `deskflow-core server`
**segfalha** nesta máquina:

```
INFO: initial settings file: /home/user/.config/Deskflow/Deskflow.conf
Segmentation fault (core dumped)      ← exit 139
```

Reproduzido duas vezes: com config isolada no scratchpad e com a config
real (telas preenchidas em `screens\N\name` + `deskflow-server.conf`
só com `links`, que é o formato do 1.27 — as telas saíram do server
config e foram para a config geral).

O ponto importante: **crasha antes de chegar ao portal.** Então não é a
lacuna de `InputCapture` do GNOME que a doc do Deskflow descreve — essa
eu descartei por teste (o `gnome.portal` declara
`org.freedesktop.impl.portal.InputCapture` e o `CreateSession` v1 é
aceito). É bug do modo servidor deste build.

Consequência prática: o Windows continua sendo o servidor, e os MX
precisam ficar no canal dele. Se um dia quiser inverter, os candidatos
são **lan-mouse** (Rust, Wayland-nativo, usa o portal InputCapture que
aqui existe — não está no apt) ou uma versão futura do Deskflow.

## Gerenciar o Windows a partir daqui

Inverter o KVM não era o caminho para isso — o Deskflow só decide qual
teclado dirige. Para operar a máquina Windows daqui:

### SSH (shell real, scriptável)

```bash
sudo powershell -ExecutionPolicy Bypass -File windows-ssh.ps1   # no Windows, como admin
ssh -i ~/.ssh/id_ed25519_windows <seu-usuario>@192.168.1.43 'hostname'
```

O [`windows-ssh.ps1`](windows-ssh.ps1) instala o OpenSSH Server, põe o
sshd em automático, abre a porta 22 no perfil Privado, define o
PowerShell como shell default e autoriza a chave
`~/.ssh/id_ed25519_windows`.

### A chave tem passphrase

Defina a sua (o comando é interativo, ninguém além de você digita):

```bash
ssh-keygen -p -f ~/.ssh/id_ed25519_windows
```

A **chave pública não muda** ao adicionar passphrase — verificado com
`ssh-keygen -y`. Então o `windows-ssh.ps1` continua válido, e dá para
pôr a passphrase antes ou depois de rodá-lo, em qualquer ordem.

Para a passphrase não virar incômodo, o `~/.ssh/config` já traz:

```
Host windows-host
    HostName 192.168.1.43
    IdentityFile ~/.ssh/id_ed25519_windows
    IdentitiesOnly yes
    AddKeysToAgent yes
```

Com `AddKeysToAgent yes` e o `gcr-ssh-agent` do GNOME
(`SSH_AUTH_SOCK=/run/user/1000/gcr/ssh`), a passphrase é pedida **uma
vez por sessão** e fica no agente. Depois disso `ssh windows-host` entra
direto — e qualquer automação (inclusive um agente rodando aqui) usa a
chave pelo agente, sem nunca ver a passphrase.

> ⚠️ A pegadinha que o script resolve: para contas no grupo
> **Administrators**, o sshd do Windows **não lê** `~/.ssh/authorized_keys`
> — lê `C:\ProgramData\ssh\administrators_authorized_keys`, e **ignora
> o arquivo em silêncio** se a ACL não estiver restrita. Por isso o
> `icacls /inheritance:r` com permissão só para Administrators e SYSTEM.

### Arquivos antigos

Nada a instalar: o `gvfs-backends` já está aqui, então o Nautilus abre
`smb://192.168.1.43` direto em *Outros locais*. Para montar em
caminho fixo, `cifs-utils` + fstab.

### Desktop do Windows numa janela

`freerdp3-x11` ou `remmina` (ambos no apt). Exige Windows **Pro** — o
Home não tem servidor RDP. E RDP toma a sessão local, então não combina
com uso simultâneo nem com jogo; para jogar, o caminho é ir fisicamente
(o Deskflow já resolve o input) ou Sunshine/Moonlight.

## Clipboard: por que o Ctrl+V não colou

O clipboard do **Deskflow não funciona no Wayland** aqui (`libportal`
0.9.1 < 0.10). Quem faz esse trabalho é o par **GSConnect ↔ KDE
Connect**, e ele estava pela metade:

- GSConnect estava instalado mas **não habilitado** (state
  `INITIALIZED`, nada na 1716). Resolvido com
  `gnome-extensions enable gsconnect@andyholmes.github.io` — pegou
  **sem relogin**, agora `ACTIVE` e escutando 1716/tcp+udp.
- KDE Connect **não existia no Windows**. Instalado via
  `winget install KDE.KDEConnect` (26.08.2, baixado de download.kde.org
  com hash verificado), em `%LOCALAPPDATA%\Programs\KDE Connect\bin`,
  com atalho de autostart para `kdeconnect-indicator.exe` criado em
  `shell:startup`.

Depois de aberto no Windows, faltava ainda a terceira causa: **o
instalador do KDE Connect não cria regra de firewall**. O `kdeconnectd`
(PID 5584) estava escutando 1716 em TCP e UDP, e `Get-NetFirewallRule
-DisplayName "*KDE*"` não devolvia nada — ele falava e ninguém ouvia.
Mesma causa do Deskflow no início. Resolvido daqui, por SSH:

```powershell
New-NetFirewallRule -DisplayName "KDE Connect (TCP)" -Direction Inbound `
  -Action Allow -Protocol TCP -LocalPort 1716-1764 -Profile Private,Domain
New-NetFirewallRule -DisplayName "KDE Connect (UDP)" -Direction Inbound `
  -Action Allow -Protocol UDP -LocalPort 1716-1764 -Profile Private,Domain
```

Alcance verificado **nas duas direções**: 1716 aberta daqui para lá, e
`Test-NetConnection 192.168.1.44 -Port 1716` → `True` de lá para cá
(o `install.sh` já havia liberado no ufw).

### Pareado, e em modo unidirecional de propósito

Estado verificado por DBus:

```
Name: 'windows-host'   Connected: true   Paired: true
EncryptionInfo: 'Verification key: 4BCF0AF8'
last-connection: 'lan://192.168.1.43:1716'
```

O plugin de clipboard tem duas chaves, **ambas `false` por default** —
é por isso que o Ctrl+V não colava mesmo pareado:

| chave | valor aqui | efeito |
|---|---|---|
| `receive-content` | **`true`** | clipboard do Windows chega aqui |
| `send-content` | `false` | o clipboard **daqui não sai** |

Assimétrico de propósito: o Bitwarden roda nesta máquina, então deixar o
envio desligado contém o
[risco do gerenciador de senhas](#clipboard-automático--gerenciador-de-senha)
sem perder o caso de uso real ("copiei lá, colei aqui"). Para espelhar
nas duas direções, ligue `send-content` ciente disso.

> ⚠️ **`gsettings set` não persistiu** nessa chave — reportava `true` no
> `get` mas o `dconf read` voltava vazio e `clipboard/` nem aparecia em
> `dconf list`. Não tenho explicação limpa. O que funcionou:
>
> ```bash
> ID=04d5cb9ad5c9460c943b5465be66bc86
> dconf write /org/gnome/shell/extensions/gsconnect/device/$ID/plugin/clipboard/receive-content true
> ```
>
> Confira sempre com `dconf read`, não com `gsettings get`.

### Testar clipboard por SSH não funciona

`Set-Clipboard` via SSH roda em outra sessão: o `Get-Clipboard` da mesma
sessão devolve o valor, mas o `kdeconnectd` da sessão interativa nunca o
vê. Mesmo isolamento que impede lançar GUI por SSH. O teste tem que ser
manual, copiando na própria máquina.

> `kdeconnect-cli --list-devices` por SSH **não serve** de diagnóstico:
> roda fora da sessão dbus do usuário e devolve `0 dispositivos` com um
> erro de window handle do Qt, mesmo com tudo funcionando.

## Compartilhar tela: já existe, não precisa construir

### Ubuntu visto do Windows — zero software extra

O `gnome-remote-desktop` 50.2 **já está instalado e rodando** aqui, com
RDP **enabled** na 3389, certificado TLS próprio e credenciais
configuradas. Verificado: escutando, e o Windows alcança
(`Test-NetConnection … -Port 3389` → `True`).

Do Windows: `mstsc` → `192.168.1.44`. Cliente da própria Microsoft.

```bash
grdctl status          # ver estado, porta, fingerprint TLS
grdctl rdp disable     # desligar, se não quiser isso exposto na LAN
```

> ⚠️ Isso é um servidor RDP com autenticação por senha aberto para toda
> a LAN, na máquina principal. Saiba que está ligado — e desligue se não
> for uso consciente.

### Windows visto do Ubuntu

O Windows é **10 Pro**, então tem servidor RDP — só está desligado
(3389 fechado). Ligando lá, `remmina` ou `freerdp3-x11` daqui resolvem.
Ressalva: RDP toma a sessão local, não serve para uso simultâneo nem
para jogo.

### Para jogo / baixa latência

**Sunshine** (host no Windows) + **Moonlight** (cliente aqui).
Encode por hardware, latência de uma ordem de magnitude menor que RDP
ou RustDesk.

### Por que não construir um RustDesk

Seria: captura (PipeWire portal no Wayland, DXGI no Windows), encode
H.264/AV1, transporte com tuning de latência, travessia de NAT, injeção
de input (libei), e a camada de cripto/auth. É o escopo de um produto,
não de um projeto de fim de semana — e o RustDesk é open source e
**self-hostável** (`hbbs`/`hbbr`), o que já atende "próprio e seguro"
sem escrever nada. Em LAN você nem precisa de relay: conexão direta.

## `logid` — o control plane (Rust)

Em [`logid/`](logid/). Binário de 929 KB, sem runtime.

```bash
cargo build --release          # dentro de logid/
install -m755 target/release/logid ~/.local/bin/logid
systemctl --user enable --now logid.service
```

Config em `~/.config/logi/config.toml`, gravada no primeiro boot se não
existir. **Config inválida não vira default silencioso**: o daemon
recusa e sai com código 1.

| Rota | O quê |
|---|---|
| `GET /healthz` | vivo? **0,19 ms** — não toca em rede |
| `GET /api/status` | estado das máquinas (~2 s; o SSH domina) |
| `GET /api/config` | config efetiva |
| `PATCH /api/config` | alterar; valida, grava, e só então aplica |
| `GET /api/devices` | inventário HID++ (receptor, slots, online/offline) |
| `POST /api/focus` | passar o foco de input — **dry-run por padrão** |
| `GET /` | o painel |

O `/healthz` existe por um erro real: o health check do Electron batia
no `/api/status` com 800 ms de timeout, e esse endpoint leva 2 s porque
abre SSH para buscar uptime e RAM. Falhava sempre em cache frio.

### PATCH valida de verdade

```
{"campo_que_nao_existe":1}   -> 400  campo desconhecido: ...
{"refresh_seconds":"texto"}  -> 422  invalid type: string
{"port":0}                   -> 422  port invalida
{"hosts":[]}                 -> 422  precisa de ao menos um host
nao-e-json                   -> 400  JSON invalido
[1,2,3]                      -> 400  esperado um objeto JSON
```

Grava em disco **antes** de aplicar em memória: se o disco recusar,
nada muda. E a página passa a servir o novo `refresh_seconds` sem
reiniciar o daemon.

### `POST /api/focus` — a peça que não existe em mais nada

Passa o foco de input para outra máquina, escolhendo entre a rota
**física** (HID++ 0x1814 manda o dispositivo pular de canal
Easy-Switch) e a rota **LAN** (Deskflow). Ver
[`specs/005`](specs/005-input-switch.md).

**Dry-run por padrão.** Sem `"confirm": true` só devolve o plano, porque
a rota física é **irreversível do lado de origem**: depois do
`ChangeHost` este receptor não alcança mais o dispositivo.

```bash
curl -X POST -d '{"host":"windows-host"}' localhost:8787/api/focus
```

Hoje o plano devolve três bloqueios reais, e é isso que ele deve fazer:

```
[ok   ] host                     windows-host (windows-host.local)
[ok   ] alcance                  windows-host.local responde
[ok   ] hidraw                   ACL de uaccess presente
[BLOQ ] posse do dispositivo     'MX Anywhere 3' está offline aqui
[BLOQ ] posse do dispositivo     'MX Keys Keyboard' está offline aqui
[BLOQ ] dead-man's switch        o agente do Windows ainda não existe
```

Com `"confirm": true` e bloqueios pendentes, responde **409** e não
executa. A rota LAN é honesta sobre o próprio limite: o Deskflow troca
de tela por movimento do cursor ou hotkey **no servidor**, não por
comando externo — não há como forçar foco pela LAN daqui.

E o plano traz `recovery` junto, para a GUI mostrar **antes** da troca:
botão Easy-Switch no dispositivo, teclado USB, SSH de outra máquina.

### Inventário HID++

`GET /api/devices` lê via Solaar e reporta o que importa:

```json
{ "hidraw_access": true,
  "receivers": [{ "name": "Unifying Receiver", "usb_id": "046d:C52B",
    "devices": [ {"slot": 1, "name": "MX Anywhere 3", "online": false},
                 {"slot": 2, "name": "MX Keys Keyboard", "online": false} ]}]}
```

`online: false` significa que o dispositivo está em **outro** canal — e
enquanto estiver, nenhum comando HID++ o alcança. Essa é a restrição que
define a arquitetura.

> O backend é o Solaar (parse de `solaar show`). Fragilidade conhecida e
> temporária: a fronteira do módulo foi desenhada para HID++ 0x1814
> direto sobre `/dev/hidraw` substituir sem tocar em quem chama.

#### A pegadinha do `uaccess`

Se `hidraw_access` vier `false`, a causa provável é temporal: a regra do
Solaar concede acesso por `TAG+="uaccess"`, e ela **só aplica ACL a nós
criados depois dela**. Aqui os `hidraw` nasceram 11:37 e o Solaar entrou
13:22 — resultado, `solaar show` morria em `No supported device found`.

```bash
sudo udevadm trigger --subsystem-match=hidraw --action=add
getfacl /dev/hidraw0 | grep user     # confirmar
```

### O agente Windows

[`agent/`](agent/) — Rust, cross-compilado daqui, **352 KB**, sem runtime:

```bash
cd agent
cargo +stable build --release --target x86_64-pc-windows-gnu
scp target/x86_64-pc-windows-gnu/release/logid-agent.exe \
    windows-host:'C:/Users/user/AppData/Local/logid/logid-agent.exe'
```

Toolchain: `rustup target add x86_64-pc-windows-gnu` + apt
`gcc-mingw-w64-x86-64`. O `.cargo/config.toml` do crate aponta o linker.

É **CLI, não servidor**, invocada por SSH: zero porta nova, zero auth
nova. O `logid` fala com ele pelo `ssh windows-host` que já existe.

Lido do hardware por HID++ 0x1814/0x1815:

```
slot 1 (MX Anywhere 3):     3 canais, atual = 2
slot 2 (MX Keys Keyboard):  3 canais, atual = 2
```

Ou seja **`windows-host` é o canal 2**, confirmado — não mais chute. O
canal do `ubuntu-host` continua desconhecido e **fora da config de
propósito**: mandar o dispositivo para canal errado o deixa inalcançável
das duas máquinas. Para descobrir, aperte Easy-Switch até ele aparecer
aqui e rode `solaar config <slot> change-host`.

> ⚠️ No Windows o receptor expõe **uma coleção HID por tipo de report**
> (usage 1 = curto `0x10`, 2 = longo `0x11`, 4 = muito longo). Escrever
> o tamanho errado na coleção errada faz o write falhar com
> `hidapi error:` vazio. O agente usa sempre o report longo na usage 2,
> para escrever e ler na mesma coleção. No Linux um único `hidraw`
> carrega todos os tipos — o problema não existe lá.

### Bateria e botões

```
GET  /api/controls           controles de cada dispositivo ONLINE
POST /api/controls/remap     {slot, cid, to, confirm}
```

A bateria vem por HID++ em dois features, porque dispositivos da mesma
família usam diferentes: **0x1004** (porcentagem) no MX Anywhere 3 e
**0x1000** (faixa) no MX Keys. Validada contra o `upower`, que lê pelo
driver do kernel e reporta o mesmo.

Leitura ausente usa a **última conhecida**, com `battery_age_s` — porque
o HID++ não lê bateria de quem não responde, e "5% há 2min" vale mais
que "?" numa bandeja. Fingir que é leitura de agora seria pior.

O remapeamento recusa controle que o próprio dispositivo diz não ser
reprogramável: esquerdo e direito reportam `flags=0x01`, sem o bit
`0x10`. E `to: "0"` restaura o padrão remapeando o controle **para ele
mesmo** — com zero, a escrita retorna `ok` e nada muda.

> ⚠️ Bateria baixa **quebra** enumeração longa. O teclado tem 24
> controles e a 5% não sustenta as ~50 idas e voltas; o mouse, com 7,
> completa. O erro sai por dispositivo, não derrubando a resposta.

### Três estados por serviço

Dois davam falso alarme — o SSH que não existe aqui de propósito e o
RDP do Windows desligado por escolha:

| `expect` | Significado |
|---|---|
| `up` (default) | tem que estar no ar; vermelho se cair |
| `down` | tem que estar fechada (a 24800 **aqui**: esta máquina é cliente) |
| `any` | informativo, nunca alarma |

Adicionar uma terceira máquina é um bloco `[[hosts]]` no TOML.

### GUI (Electron)

Em [`gui/`](gui/):

```bash
cd gui && npm install && ./run.sh
```

Janela nativa, bandeja, instância única, fechar esconde. **Não duplica
lógica**: o `logid` serve a UI e a GUI embrulha. Se o
`logid.service` está no ar ela reaproveita; se a porta está livre, sobe
um filho — e só então o mata ao sair.

Electron fixado em **44.9.0**. Não chute versão: eu assumi 38.x e a
linha atual é 45.x. Confira com `npm view electron time --json` e
prefira uma com ≥ 2 semanas.

> ⚠️ O host de extensões do VS Code exporta `ELECTRON_RUN_AS_NODE=1`, e
> com ela o binário do Electron roda como Node puro e **nada abre** —
> foi por isso que `electron --version` devolveu a versão do Node.
> `run.sh` e `npm start` limpam a variável.

## O IP muda — e isso quebrou tudo uma vez

Em 03/11 o Windows saiu de `192.168.1.43` para `192.168.1.42` (mesmo
MAC `00:00:5e:00:53:01`, confirmado por `ip neigh`). Lease de DHCP novo.
O estrago:

| | Antes | Resultado |
|---|---|---|
| Deskflow | `remoteHost=192.168.1.43` | **KVM morto**, log em loop de `No route to host` |
| SSH | `HostName 192.168.1.43` | `No route to host` |
| painel | `addr = "192.168.1.43"` | host marcado offline |
| GSConnect | descoberta por broadcast | **sobreviveu** — reconectou sozinho em `.102` |

A lição está na última linha: o que descobre por nome/broadcast se
curou; tudo que eu havia fixado em IP quebrou.

### Correção aplicada: nome em vez de IP

O Windows publica o próprio nome por mDNS e este Ubuntu resolve —
`getent hosts windows-host.local` → `192.168.1.42`, estável. O mecanismo:

```
/etc/nsswitch.conf:  hosts: files mdns4_minimal [NOTFOUND=return] dns
avahi-daemon:        active
libnss-mdns:         0.15.1-5
```

Então todas as configs passaram a usar **`windows-host.local`**:
`Deskflow.conf`, `~/.ssh/config` e `~/.config/logi/config.toml`.
Deskflow
reconectou (`ESTAB → 192.168.1.42:24800`) sem nenhum IP escrito.

> ⚠️ **A lista separada por vírgula do Deskflow não funciona**, apesar
> do tooltip da GUI dizer "May contain a comma separated list".
> `remoteHost=windows-host.local,192.168.1.42` fez o core abortar com
> `FATAL: No server addresses configured`. Use **um** host só.

### A armadilha do known_hosts

Trocar IP por nome fez o SSH falhar em `Host key verification failed`:
a chave estava no `known_hosts` sob o IP, não sob o nome. A correção
**não** é `accept-new` às cegas — é copiar a *mesma* chave já confiada:

```bash
KEY=$(ssh-keygen -F 192.168.1.42 -f ~/.ssh/known_hosts | grep -v '^#' | head -1 | awk '{print $2, $3}')
echo "windows-host.local $KEY" >> ~/.ssh/known_hosts
```

Fingerprints conferidas depois, idênticas:
`SHA256:w3vRoMELMTEZz1EdlfqgvXeW1e9tpXS2OzDntbtwB3Y`.

Note que `ssh-keyscan windows-host.local` **falha** com
`Temporary failure in name resolution` mesmo com o `getent` funcionando
— quirk do `mdns4_minimal` com `[NOTFOUND=return]` em consulta
AF_UNSPEC. O `ssh` em si resolve normalmente.

### O que ainda falta fazer (no roteador)

mDNS depende do avahi e de multicast funcionando. Para redundância,
crie uma **reserva de DHCP** no roteador para o MAC
`00:00:5e:00:53:01`. Aí o IP para de dançar e o nome continua
funcionando — cinto e suspensório.

## Limites que você vai sentir

- **Clipboard do próprio Deskflow não funciona no Wayland** aqui: ele
  precisa de `libportal` ≥ 0.10 e o Ubuntu 26.04 traz 0.9.1. É
  exatamente por isso que o GSConnect entra — ele faz clipboard e
  arquivos por conta própria, sem depender disso.
- **Arrastar-e-soltar arquivo** entre telas não existe no Deskflow em
  Linux (nem no Barrier/Input Leap). Transferência de arquivo vai pelo
  GSConnect/KDE Connect, que é "compartilhar para o dispositivo" em vez
  de arrastar.
- O **cursor continua visível** na tela que você deixou (cosmético,
  Wayland).
- **Ícone de bandeja do Deskflow** não funciona no GNOME Wayland —
  mantenha a janela aberta, ou coloque `deskflow-core client` no
  autostart (o 1.27 tem só dois binários: `deskflow`, a GUI, e
  `deskflow-core`, que recebe `server` ou `client` como argumento).
- **Scroll Lock ativo congela** a troca de tela (é o atalho de
  "travar na tela atual", herdado do Synergy).
- `AltGr` indo de servidor Linux → cliente Windows é problemático;
  como aqui o servidor é o Windows, não te afeta.

## Segurança e telemetria (auditado nos binários instalados)

| Pacote | Sai para a internet? | Observação |
|---|---|---|
| `deskflow` 1.27.0 | **não**, exceto update check opt-in | GPL-2. Zero telemetria/analytics/Sentry/crashpad/ativação. As únicas URLs nos binários são metadado de ícone SVG, texto da licença e o link de contributors no GitHub. |
| `solaar` | **não** | Nenhum socket de rede. Os `import socket` são só `gethostname()`, para escrever o nome da máquina no dispositivo (o rótulo que o MX Keys mostra por canal). |
| `gsconnect` | **não** | Nenhum host de terceiro no código — só referências a RFC/docs em comentários. Transporte direto entre as duas máquinas na porta 1716 da LAN. |

O update check do Deskflow (`cbAutoUpdate`, canais `stable`/`beta`,
header `X-Deskflow-Version`) é a **única** coisa do conjunto capaz de
falar com a internet. A URL não está compilada no binário — vem da
config — e o pacote não embarca `.conf`/schema default. Está inerte até
você marcar *Check for updates on startup* na GUI.

### O que de fato atravessa a rede

Não sai da sua LAN, mas **sai desta máquina**:

- **Clipboard** (GSConnect): vai direto para o Windows pelo
  `192.168.1.x`, em TLS com chaves trocadas no pareamento, sem servidor
  intermediário e sem nuvem. Nenhum terceiro vê — mas é cópia de
  verdade trafegando na rede local.
- **Teclas e movimento** (Deskflow): tudo que você digita na tela
  remota trafega como eventos de input, em TLS.

Por isso **conferir o fingerprint na primeira conexão é pra valer**, e
não só clicar em aceitar: é o que impede alguém na mesma rede se passar
pelo servidor e receber o que você digita.

### Clipboard automático × gerenciador de senha

Sync automático **espelha o que você copia do Bitwarden**. Senha
copiada aqui vira senha no clipboard do Windows, e fica lá até ser
sobrescrita.

Prefira o modo **manual** do GSConnect (empurrar/puxar sob demanda) em
vez do espelhamento contínuo. Nas preferências do dispositivo, em
*Clipboard*, desligue *Sync automatically* e mantenha só as ações
manuais — você perde a transparência, ganha não vazar senha para a
outra máquina sem querer.

## Se não conectar

```bash
# o Windows está escutando?
timeout 2 bash -c 'echo > /dev/tcp/192.168.1.43/24800' && echo aberto || echo fechado
```

Quando sondei agora, **todas** as portas do `.108` apareceram
filtradas (inclusive 445/3389) e o ICMP não respondeu, embora o ARP
resolva — ou seja, a máquina está viva e o **firewall do Windows está
barrando tudo**. Conte com o passo 2 acima ser obrigatório, não
opcional.

Para ver o handshake, rode o core direto no terminal em vez da GUI:

```bash
deskflow-core client --new-instance
```

O nível de log e o endereço do servidor saem de
`~/.config/Deskflow/Deskflow.conf` (a GUI escreve esse arquivo), então
configure uma vez na GUI e depois use a linha de comando para
diagnosticar.

## Extra: remapear botões no Linux

O `solaar` já cobre bateria, scroll e canal (`solaar config` lista os
dispositivos; `change-host` troca o canal Easy-Switch por linha de
comando — dá para amarrar a um atalho do GNOME). Para DPI/SmartShift
e remapeamento mais fino existe o **OpenLogi** (`.deb` no site deles).

⚠️ Um app por receptor: OpenLogi e Solaar brigam pelo acesso HID++.
Escolha um.
