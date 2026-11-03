# 005 — Roteamento de input (o núcleo)

É a peça que não existe em open source. Ver
[`000`](000-overview.md#a-lacuna-que-justifica-o-projeto).

## A ideia

Quando o cursor encosta na borda (ou você aperta um atalho), há **dois
jeitos** de passar o controle para a outra máquina:

| Rota | Como | Latência | Quando |
|---|---|---|---|
| **física** | manda o mouse/teclado pular de canal Easy-Switch para o receptor da outra máquina (HID++ 0x1814) | nativa — é o rádio do próprio dispositivo | a outra máquina tem receptor com o device pareado |
| **LAN** | o Deskflow transmite eventos pela rede | ~1 ms + rede, e depende de portal/libei | a outra máquina não tem receptor, ou o canal não está pareado |

O Flow faz a primeira. O Deskflow faz a segunda. **Nenhum escolhe entre
as duas.** Escolher é o produto.

A rota física é melhor quando disponível: latência nativa, funciona com
a rede caindo, e não depende de injeção de input por portal — o que
**contorna o segfault do modo servidor do Deskflow no Linux**
([`README`](../README.md#ubuntu-como-servidor-testado-e-não-funciona-1270)).
Hoje as duas máquinas têm receptor Unifying, então a rota física é
possível nos dois sentidos.

## Detecção de borda: mutter direto, não o portal

Correção de uma afirmação errada minha. Esta spec dizia "tudo por
portal, nada de hack", baseada em eu ter visto o
`org.freedesktop.portal.InputCapture` **aceitar** `CreateSession`.
Aceitar não é responder.

### O portal aceita e nunca responde

```
CreateSession  -> devolve request path
Response       -> NUNCA chega (rodada limpa, 75 s, confirmada)
backend        -> 43 requests pendentes acumulados, nenhum resolvido
no fechamento  -> "Object does not exist at .../session/icsess0"
```

Não é falta de implementação: o `xdg-desktop-portal-gnome` tem
`InputCaptureDialog` com switch de permissão, `AddBarrier`,
`ClearBarriers`, `GetZones`, `ConnectToEIS`. O que ele não consegue é
**exibir o diálogo de consentimento** —
`Failed to associate portal window with parent window`. Uma CLI sem
janela não tem o que passar em `parent_window`, e o pedido fica
pendurado para sempre. O permission store confirma: há tabela para
`remote-desktop` e `screencast`, **nenhuma** para `input-capture`.

### `org.gnome.Mutter.InputCapture` funciona, e é síncrono

O backend do portal é só um intermediário para um serviço do mutter que
está no barramento, servido pelo gnome-shell, e atende chamada direta —
sem Request/Response, sem diálogo:

```
org.gnome.Mutter.InputCapture
  CreateSession(u capabilities) -> o session_path
  SupportedCapabilities = 15

org.gnome.Mutter.InputCapture.Session
  GetZones()                     -> (u serial, a(uuii) zones)
  AddBarrier(u serial, (iiii))   -> u id
  ClearBarriers() / Enable() / Disable() / Close()
  ConnectToEIS()                 -> h fd
  Release(a{sv} options)
  sinais: Activated(u barrier_id, u activation_id, (dd) cursor)
          Deactivated(u activation_id), ZonesChanged, Disabled, Closed
```

### Validado com hardware (03/11)

```
CreateSession(2)   -> /org/gnome/Mutter/InputCapture/Session/u5
GetZones()         -> serial=0, zones=[(2560, 1440, 0, 0)]
AddBarrier(x=2560) -> id=1
ConnectToEIS()     -> fd=8
Enable()           -> ok

*** ACTIVATED *** barreira=1 activation=1 cursor=(2560.0, 813.23)
[Release] ponteiro devolvido
[Deactivated] (1,)
*** ACTIVATED *** barreira=1 activation=2 cursor=(2560.0, 813.23)
[Release] ponteiro devolvido
```

**Dá para usar só a detecção** e devolver o ponteiro na hora, sem
precisar capturar nem encaminhar input via libei. É o gatilho da rota
física saindo de graça.

### Três coisas que custaram tentativa

**A barreira vai na FRONTEIRA da zona, não na última coluna dela.**
Com `x = x + w - 1` (2559) o mutter recusa:
`AccessDenied: Line overlaps with monitor region`. Tem que ser
`x = x + w` (2560).

**`ConnectToEIS` antes do `Enable`.** Não confirmei que seja
obrigatório — entrou no teste para eliminar a hipótese e o evento
chegou com ele presente. Fica como ordem conhecida-boa até alguém medir
sem.

**Movimento injetado vs. real.** O primeiro teste deu timeout com os
dispositivos no canal do Windows: o ponteiro aqui era injetado pelo
Deskflow via libei, e movimento injetado pode não atravessar barreira do
compositor. O evento só veio com o dispositivo **no canal local**. Isso
não é limitação — é exatamente o modo B, onde o device é local por
definição.

## Pré-requisito: acesso ao hidraw

HID++ precisa abrir `/dev/hidraw*`. A regra do Solaar
(`/usr/lib/udev/rules.d/60-solaar.rules`) concede via `TAG+="uaccess"`,
que vira ACL para o usuário logado.

**Pegadinha já paga:** os `hidraw` nasceram às 11:37 e o Solaar foi
instalado às 13:22 — a regra não existia na criação dos nós, nenhuma ACL
foi aplicada, e o `solaar config` morria em `No supported device found`
com `getfacl /dev/hidraw0` mostrando só `user::rw-` (root).

```bash
sudo udevadm trigger --subsystem-match=hidraw --action=add   # ou religar o receptor
getfacl /dev/hidraw0 | grep user                           # confirmar a ACL
```

O `logid` deve **checar isso no startup** e dizer na API/GUI quando
faltar, em vez de falhar obscuro na hora da troca.

Mapeamento nesta máquina:

| hidraw | dispositivo |
|---|---|
| `hidraw0` | `046D:C52B` — receptor Unifying |
| `hidraw2` | `046D:4090` — MX Keys |
| `hidraw3` | `046D:408A` — MX Anywhere 3 |

## Restrição descoberta: só quem detém o device pode empurrá-lo

Testado: com o mouse no canal do Windows, `solaar config 1` falha com
`no online device found matching '1'`, e `solaar show` lista os dois
devices como `offline`.

O motivo é físico: o comando HID++ sai do receptor por rádio até o
dispositivo. Se ele está ouvindo **outro** receptor, não há como
alcançá-lo. Ou seja:

> A troca de canal só pode ser emitida pela máquina que **está com o
> dispositivo naquele momento**.

Isso torna a arquitetura obrigatoriamente **simétrica**: o control
plane não executa a troca, ele **pede ao detentor atual** que execute.
Quem está sem o device não tem como trazê-lo de volta pela rota física.

### O lado Windows: resolvido

O Solaar é Linux-only, então o Windows precisava de HID++ próprio.
Implementado em [`agent/`](../agent/): Rust, cross-compilado do Ubuntu
(`x86_64-pc-windows-gnu` + mingw do apt), **352 KB**, sem runtime.

É uma **CLI, não um servidor**, invocada por SSH. Reaproveita a chave
que já existe: zero porta nova, zero auth nova, zero superfície de rede.

```
logid-agent devices                      # slots, nº de canais, canal atual
logid-agent host-info <slot>
logid-agent set-host <slot> <n> --confirm
logid-agent input-age                    # ms desde o último input
logid-agent guard <slot> <n> <segundos>  # dead-man's switch
logid-agent raw <slot> <feature> <func>  # sondagem de protocolo
```

HID++ via `hidapi` (integrar em vez de reimplementar: enumeração e I/O
de HID no Windows são ~300 linhas de FFI unsafe já resolvidas lá).

#### A pegadinha das coleções HID no Windows

Primeira tentativa: write falhava com `hidapi error:` vazio. Causa:
no Windows o receptor Logitech expõe **uma coleção HID por tipo de
report** — usage 1 = curto (`0x10`, 7 bytes), usage 2 = longo (`0x11`,
20 bytes), usage 4 = muito longo. Eu abria a primeira da lista (usage 2)
e escrevia um report curto nela. Tamanho errado, write recusado.

Solução: usar **sempre o report longo na coleção usage 2**, mesmo para
funções que caberiam no curto. Assim escrita e leitura ficam na mesma
coleção — a alternativa seria abrir duas e correlacionar entre elas.

No Linux não existe esse problema: um único `hidraw` carrega todos os
tipos de report.

### Lido do hardware (03/11)

```
slot 1 (MX Anywhere 3):     3 canais, atual = 2
slot 2 (MX Keys Keyboard):  3 canais, atual = 2
0x1815 func 0:  13 04 03 01   -> numHosts=3, currentHost=1 (0-based)
0x1815 func 1:  os três canais com status=1 (pareado)
```

Então **`windows-host` é o canal 2**, confirmado, e os dispositivos
conhecem três hosts — todos pareados.

O canal do `ubuntu-host` **ainda não foi lido**, e não entra na config
como chute: mandar o dispositivo para um canal errado o deixa
inalcançável das duas máquinas. A descoberta sem risco é uma tecla:
apertar Easy-Switch até o dispositivo aparecer aqui, e então
`solaar config <slot> change-host` reporta o canal atual.

> Tentei ler os nomes dos canais por `0x1815` func 2 e o layout não
> rendeu (`02 80 00…`, sem ASCII). Abandonei: a função provável de
> escrita fica ao lado da de leitura no mesmo feature, e sondar índice
> de função às cegas podia **renomear** um host. Não valia o risco para
> uma informação que uma tecla resolve.

### Não existe rollback deste lado

Correção de um erro de desenho meu: a primeira versão desta spec dizia
"se o alvo não confirmar, voltar ao canal anterior". **Isso é
impossível.**

O `ChangeHost` viaja por rádio até o dispositivo. Depois que ele troca
de canal, este receptor **não o alcança mais** — comprovado por
`solaar config 1` falhando com `no online device found` justamente
porque o device está no canal do Windows. Então a troca é
**irreversível do lado de origem**, e não há ponto de retorno: no
instante seguinte ao comando, estamos cegos e sem input.

Duas consequências de projeto:

**1. O pre-flight substitui o rollback.** Toda verificação acontece
*antes* do ato irreversível, porque depois não há o que fazer. Por isso
`POST /api/focus` é **dry-run por padrão** e só executa com
`"confirm": true` — mesmo princípio da regra de disparo destrutivo do
outro repo: a ação é irreversível, então preview é obrigatório.

**2. O dead-man's switch mora no ALVO.** Quem pode devolver o
dispositivo é quem passa a detê-lo. O agente do alvo deve:

- esperar input por N segundos depois de receber o device;
- se nada chegar, emitir `ChangeHost` de volta por conta própria.

Isso inverte a responsabilidade: a origem não tenta se salvar, o
destino a resgata. É também a razão mais forte para existir um agente
no Windows em vez de só SSH — e enquanto ele não existir, a rota física
é **de ida só**, e a volta é pelo botão físico do dispositivo.

### Recuperação sem input

Se os dois lados falharem, o usuário está sem teclado e sem mouse. Os
caminhos que sobram, em ordem:

1. botão Easy-Switch no próprio dispositivo (físico, sempre funciona);
2. um teclado USB qualquer;
3. SSH de outra máquina para `ubuntu-host`.

A GUI tem que mostrar o item 1 **antes** de qualquer troca, não depois.

## Config

```toml
[input]
prefer = "physical"        # "physical" | "lan" | "auto"
confirm_timeout_ms = 3000

[input.border]
enabled = true
edge = "left"              # borda desta máquina que leva ao Windows
hold_ms = 250              # evita troca acidental ao passar raspando

[input.shortcut]
enabled = true
accel = "<Super><Alt>Right"

[[devices]]
name = "MX Anywhere 3"
hidraw_id = "046D:408A"
hosts = { "ubuntu-host" = 1, "windows-host" = 2 }   # canal Easy-Switch

[[devices]]
name = "MX Keys"
hidraw_id = "046D:4090"
hosts = { "ubuntu-host" = 1, "windows-host" = 2 }
```

O mapa `hosts` é o que o Solaar não tem e o Flow guarda na nuvem.
Aqui mora num TOML seu.

## Validado ponta a ponta (03/11)

Ciclo completo, nas duas direcoes, com hardware real:

```
# Windows -> Ubuntu, pelo agente via SSH
logid-agent set-host 1 1            -> recusa: falta --confirm
logid-agent set-host 1 1 --confirm  -> {"ok":true,"slot":1,"host":1}
logid-agent set-host 2 1 --confirm  -> {"ok":true,"slot":2,"host":1}
   Ubuntu:  slot 1 e 2 online=True
   Windows: nenhum slot online

# Ubuntu -> Windows, pelo logid
POST /api/focus {"host":"windows-host","confirm":true}
   executado: true
   MX Anywhere 3:    slot 1 -> canal 2
   MX Keys Keyboard: slot 2 -> canal 2
   Ubuntu:  online=False
   Windows: slot 1 e 2 ONLINE canal=2
```

**É o Logitech Flow, funcionando, sem nuvem e sem conta.** A peça que
nenhum projeto aberto tinha.

### `input-age` por SSH não serve — comprovado

```
$ ssh windows-host logid-agent input-age
{"idle_ms":704577031,"ok":true}        # ~8,15 dias
```

8,15 dias é o **uptime** da máquina. A sessão do SSH nunca viu input,
então `GetLastInputInfo` devolve o tempo desde o boot. Isso confirma por
medida o isolamento de sessão descrito em
[`002`](002-ssh.md#limite-estrutural-isolamento-de-sessão): o `guard`
**não pode** ser disparado por SSH, tem que estar residente na sessão
interativa (atalho em `shell:startup`).

## Dois modos de trabalho, agora que a rota física existe

| | Dispositivos em | Como troca | Preço |
|---|---|---|---|
| **A** (atual) | Windows | cursor na borda, via Deskflow | input do Ubuntu depende do Windows ligado |
| **B** (novo) | Ubuntu | `POST /api/focus` | latência nativa, sem depender do Windows — mas falta o gatilho de borda |

O modo B é o melhor para quem tem o Ubuntu como máquina principal, e
depende só da detecção de borda pelo portal `InputCapture` para ficar
indistinguível do Flow.

## Implementado em Rust e medido (03/11)

[`logid/src/border.rs`](../logid/src/border.rs), via crate `dbus`
(libdbus), **não** zbus: medido, zbus levava a árvore de 5 para **189
crates** arrastando runtime assíncrono para fazer seis chamadas D-Bus.
Com `dbus 0.9.12` ficou em **40**, ao custo de `libdbus-1-dev` do apt.
Descartado o `zbus 6.0.0` por ter 17 dias — major novo demais.

```
border: barreira 1 na borda right (2560, 0, 2560, 1440), eis=sim
border: vigiando borda 'right' -> windows-host
border: ACTIVATED barreira=1 activation=1 cursor=(2560.0, 555.18)
```

### Três bugs que só o teste ao vivo mostrou

**1. O solaar reporta falha numa troca que funcionou.**

```
18:05:48  ACTIVATED
18:06:23  border: MX Anywhere 3: FALHOU --      ← mensagem VAZIA, 35 s depois
```

Os dispositivos estavam no Windows, canal 2: **a troca tinha dado
certo.** Numa troca bem-sucedida o dispositivo desaparece no meio do
comando, o solaar trava até o timeout e devolve erro vazio. Eu já
tratava esse caso no agente Rust (`"sem resposta é o caso ESPERADO"`) e
esqueci no caminho via solaar.

Correção: **o exit code do solaar não é fonte de verdade.** Verificamos
pelo efeito observável — se o slot saiu deste receptor, trocou.

**2. O handler bloqueava o loop do D-Bus por 35 s.**

A troca roda em thread própria agora. Antes, nenhum sinal era processado
durante ela.

**3. Devolver o ponteiro na borda re-disparava a barreira em loop.**

Chegamos a `activation=54` com o cursor parado em x=2560: cada `Release`
o devolvia exatamente na barreira. O `Release` aceita `cursor_position`,
então recuamos **24 px** para dentro. Depois disso, zero ativações
espúrias.

O cooldown subiu de 3 s para 10 s, porque a troca mais a verificação por
efeito leva ~5 s e o segundo evento entrava antes do primeiro terminar.

### A troca não é atômica

Mouse e teclado são comandos independentes, segundos cada um. Observado:
o mouse saiu e o teclado ficou por alguns segundos — **estado real e
transitório**, não erro de leitura (suspeitei do parser e estava errado;
a API e o `solaar show` concordaram no mesmo instante).

O risco de verdade é o segundo falhar e o estado dividido ficar
**permanente** — e não há rollback para quem já foi. Por isso
`Outcome.partial` agora grita:

```
ESTADO DIVIDIDO: 1 trocaram, mas 1 ficaram aqui (MX Keys Keyboard).
Nao ha rollback para quem foi -- use o botao Easy-Switch no que ficou.
```

## O par simétrico: `watch-edge` no Windows

[`agent/`](../agent/) ganhou `watch-edge`, que fecha o ciclo do modo B:

```
logid-agent watch-edge left 1,2 1 [hold_ms]
```

Vigia a borda e, ao encostar, manda os slots para o canal do alvo.

**No Windows isso é muito mais simples que no Linux**: não há portal,
não há barreira de compositor, não há diálogo de consentimento. Ler
`GetCursorPos` e `GetSystemMetrics` resolve. Usamos a área **virtual**
(`SM_XVIRTUALSCREEN` e família) em vez de `SM_CXSCREEN`, que só reporta
o monitor primário.

Diferenças de projeto em relação ao lado GNOME:

| | Linux (`border.rs`) | Windows (`watch-edge`) |
|---|---|---|
| detecção | barreira do compositor via D-Bus | polling de `GetCursorPos` a 30 ms |
| permanência | não existe no mutter; usamos cooldown | `hold_ms`, default 250 ms |
| recuo do ponteiro | necessário (`Release` com `cursor_position`) | desnecessário: o cursor para sozinho na borda |

O `hold_ms` do lado Windows é melhor que o cooldown do lado Linux:
exigir permanência evita a troca por raspão, em vez de só suprimir
repetição depois do fato. Vale portar a ideia para o `border.rs`.

### Precisa da sessão interativa

Medido: rodando por SSH, o agente reporta
`area virtual (0,0 1024x768)` — a tela falsa da sessão não-interativa,
não o monitor real. Mesmo isolamento que já invalidou `input-age` por
SSH. Por isso o atalho vai em `shell:startup`, com
`WindowStyle = 7` (minimizado), e **não** como serviço:

```powershell
powershell -ExecutionPolicy Bypass -File windows-autostart.ps1 -WatchEdge
```

## Em aberto

- Canal do `ubuntu-host`: uma tecla de Easy-Switch resolve.
- Detecção de borda pelo portal `InputCapture` — API existe no GNOME 50
  (backend declara e `CreateSession` é aceito), mas é D-Bus + libei, e a
  v1 pode não bastar.
- `guard` ainda não roda residente: precisa de atalho em `shell:startup`
  para ficar na sessão interativa, já que `GetLastInputInfo` não vê input
  de outra sessão.
- Latência real da troca HID++ ponta a ponta — medir antes de prometer.
- E se o mouse estiver no canal de uma máquina desligada? Só o botão
  físico resolve, e a GUI tem que dizer isso.
