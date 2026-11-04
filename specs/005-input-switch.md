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

### Permanência no lado Linux: densidade de ativações

Portado, mas por outro mecanismo — o mutter **não tem** dwell, a barreira
dispara no contato. Aproximamos pela **densidade de ativações**:

| | |
|---|---|
| `HOLD_WINDOW` | 400 ms |
| `HOLD_HITS` | 3 ativações na janela |

A medição que virou mecanismo: com o cursor parado em x=2560 chegamos a
`activation=54`, porque cada `Release` o devolvia na barreira. Esse
re-disparo é exatamente o sinal de que a pessoa está **insistindo** na
borda; um raspão de passagem gera uma ativação só.

Então o `Release` passou a ser condicional:

- **enquanto coleta** (hits < 3): devolve **sem** recuo, de propósito, para
  que insistir continue re-disparando;
- **ao decidir** (hits ≥ 3): devolve **recuado** 24 px, zera a contagem e
  arma o cooldown de 10 s.

O cooldown também vem antes da coleta: dentro dele nem contamos, só
devolvemos recuado. Isso evita que a contagem suba durante os ~5 s da
troca anterior.

A inversão é o detalhe elegante: o churn que eu primeiro tratei como bug
virou o sinal de intenção.

### Precisa da sessão interativa

Medido: rodando por SSH, o agente reporta
`area virtual (0,0 1024x768)` — a tela falsa da sessão não-interativa,
não o monitor real. Mesmo isolamento que já invalidou `input-age` por
SSH. Por isso o atalho vai em `shell:startup`, com
`WindowStyle = 7` (minimizado), e **não** como serviço:

```powershell
powershell -ExecutionPolicy Bypass -File windows-autostart.ps1 -WatchEdge
```

## O recuo do ponteiro não existe: o mutter ignora

Medido em teste real, e isso derrubou o desenho anterior:

```
300+ ativacoes, todas com cursor=(2560.0, ...)   <- x NUNCA mudou
690 linhas de "em cooldown, ignorando"
```

O `Release` **aceita** `cursor_position` e **não move o ponteiro**. Com o
cursor encostado na borda, cada Release o devolvia ali e a barreira
re-disparava — 300 vezes num segundo, inundando o journal.

### A estratégia que funciona: desarmar a barreira

Em vez de mover o ponteiro, `Disable()` na decisão e `Enable()` quando o
cooldown vence. Sem barreira não há evento, independente de onde o
ponteiro esteja.

O `Enable` do rearme roda no **loop principal**, não na thread da troca:
a sessão pertence àquela conexão D-Bus, e outra conexão não a controla.
O estado compartilhado é um `reenable_at: Option<Instant>`.

> Isto também simplificou o `release()`: sem opções, sem fallback. A
> função inteira virou três linhas.

## `watch-edge` confundia ausência com falha

Reportado pelo uso real:

```
sem resposta para feature 0x00 func 0x00 ... switch_failed
```

Encostar na borda do Windows com os dispositivos **em outra máquina**
fazia o agente tentar `set_host` e falhar no `feature_index` — que é
ausência, não falha. Agora ele checa presença antes:

- nenhum slot presente → log claro e cooldown de 10 s, sem tentar;
- parte presente → envia só esses e **avisa do estado dividido**.

### Slot é por receptor, e não coincide entre máquinas

Descoberto ao reunir os dispositivos: o Windows tinha o **slot 1 online**
e o slot 2 vazio, enquanto aqui os dois existem. `set-host 2` de lá
falhou por isso, não por bug.

A config do `logid` já resolve por **nome** de dispositivo. O agente
ainda recebe números crus na linha de comando (`watch-edge left 1,2 1`),
e a checagem de presença o faz tolerar divergência — mas o certo é ele
ler `DEVICE NAME` (HID++ 0x0005) e casar por nome também. Fica em aberto.

## O solaar é caro demais para o caminho quente — medido

A troca **funciona**, mas levou **105 segundos** no teste real:

```
18:51:18  border: barreira desarmada; trocando...
18:53:05  border: MX Anywhere 3: slot 1 saiu deste receptor (canal 2)
18:53:05  border: MX Keys Keyboard: slot 2 saiu deste receptor (canal 2)
```

Causa medida:

```
solaar show  ->  5,9 s  /  15,3 s  (segunda medida sob contencao)
solaar show 1 ->  5,5 s            (por dispositivo, nao ajuda)
```

E o `change_host` chama isso **4× por dispositivo** (uma antes, três de
verificação), × 2 dispositivos, mais o `survey()` do `plan()` e o SSH
para o agente. Daí os 105 s.

Agrava: o **GUI do Solaar roda em paralelo** (`solaar --window=hide`),
e dois donos do mesmo receptor é exatamente a contenção que a doc do
OpenLogi descreve.

### A saída: HID++ direto, como no Windows

`hidapi` **funciona no Linux** (backend hidraw; precisa de
`libudev-dev`). E a enumeração mostrou mais do que o esperado:

```
pid=c52b  /dev/hidraw0  usage_page=0xff00   <- o receptor
pid=4090  /dev/hidraw2  usage_page=0xff00   <- MX Anywhere 3, no proprio
pid=408a  /dev/hidraw3  usage_page=0xff00   <- MX Keys, no proprio
```

Cada dispositivo pareado tem **nó hidraw próprio** com coleções HID++.
Consequências:

1. Dá para falar direto com o dispositivo (`device_index` 0xFF) em vez
   de indexar slot no receptor — o que **resolve a divergência de
   numeração** entre máquinas.
2. "Está online?" vira um ping HID++ de milissegundos, não um
   `solaar show` de 6-15 s.
3. O código do protocolo já existe em [`agent/src/hidpp.rs`](../agent/src/hidpp.rs)
   e é agnóstico de plataforma: vira crate compartilhado.

Isso era o plano desde o início ("a fronteira deste módulo foi desenhada
para que uma implementação de HID++ 0x1814 sobre /dev/hidraw substitua o
backend sem tocar em quem chama"). O custo medido do solaar tornou a
migração obrigatória, não opcional.

## Migrado para HID++ direto — medido antes e depois

O `solaar` saiu do caminho quente. O protocolo virou crate compartilhado
em [`hidpp/`](../hidpp/), usado pelo `logid` **e** pelo `agent` — uma
cópia só, testada contra hardware nos dois sistemas.

| | antes (solaar) | depois (HID++) |
|---|---|---|
| troca de canal, ponta a ponta | **105 s** | **13,8 s** |
| confirmação de que o device saiu | até 4× `solaar show` | ping de **300 ms** |
| `/api/devices` | 6-15 s | **1,5 s** |
| probe do agente por SSH | 6,4 s | **2,06 s** |
| ping de presença | — | **22 a 81 ms** |

### O que o HID++ deu de brinde

**Nome lido do dispositivo** (feature 0x0005), não do solaar. E os nomes
**divergem**: o solaar dizia `MX Keys Keyboard`, o dispositivo se chama
`MX Keys Wireless Keyboard`. A config precisou ser corrigida — confira
com `cargo run --release --bin probe` no crate.

**Latência por dispositivo** exposta na API (`ping_ms`), que não existia.

**Canal atual** de cada um, sem custo extra.

### Três defeitos que a velocidade nova revelou

**1. Dispositivo offline desaparecia da API.** O HID++ não lê o nome de
quem não responde, então a lista ficava vazia e a GUI em branco —
regressão frente ao solaar, que listava offline com nome. Resolvido com
cache de slot→nome persistido em `~/.local/state/logi/slots.json`:
runtime descartável, não config.

**2. `executed: true` com zero resultados.** Com a troca tão mais
rápida, a vigilância de borda passou a ganhar a corrida contra a API: o
pre-flight passava e, nos segundos até a execução, outra travessia já
tinha levado tudo. Agora as trocas são **serializadas** por mutex
global, e a elegibilidade é reavaliada **dentro** do lock.

**3. O texto do plano ainda citava `solaar config`.** Passou a dizer o
que de fato faz: `HID++ 0x1814 setCurrentHost`.

### Os testes trocaram de assunto

Os 5 testes antigos cobriam o parser da saída do `solaar`, que deixou de
existir. Em vez de aceitar a queda para zero, escrevi **7** sobre as
funções puras que sobraram: `barrier_line` nas quatro bordas, com offset
de monitor secundário e borda inválida; e o parsing de config —
`Expect` default, `down`/`any`, e o mapa de canais por nome.

### O gargalo que sobrou

Dos 13,8 s, o pre-flight domina, e dentro dele o SSH ao agente. Com o
agente migrado isso caiu para ~2 s, então a troca deve ficar perto de
**9 s**. O resto é latência de SSH, não de HID++.

## Teclado segue o mouse: reconciliador

[`logid/src/follow.rs`](../logid/src/follow.rs). Um dispositivo é
**líder** e os demais de `[[devices]]` seguem o canal dele, reavaliado a
cada N segundos.

```toml
[input.follow]
enabled = false              # desligado por padrão
leader = "MX Anywhere 3"     # nome HID++
interval_seconds = 5
```

Resolve o estado dividido na raiz, e — o que a borda não consegue —
**funciona quando a troca foi manual, no botão Easy-Switch**. O `border`
só vê travessia de cursor; o reconciliador vê o estado.

### A assimetria do hardware manda no desenho

Só quem detém o dispositivo pode empurrá-lo, então a ação depende de
onde o seguidor está:

| seguidor está | quem age |
|---|---|
| aqui | nós, por HID++ local |
| num par | pedimos ao agente dele, por SSH |
| em nenhum conhecido | nada — pode ser o MacBook, sem daemon |

### Validado, no caminho difícil

Testei a direção que passa pelo par, não a local:

```
# so o lider volta para o canal 1; o teclado fica no 2
ssh windows-host logid-agent set-host 2 1 --confirm

follow: MX Keys Wireless Keyboard: pedi a windows-host que o mandasse para o canal 1
-> MX Anywhere 3              online=True canal=1
-> MX Keys Wireless Keyboard  online=True canal=1
```

Agiu **uma vez** e sossegou — mais que isso seria loop.

### Casar por nome, não por canal

Primeira versão localizava o dispositivo no par comparando o **canal
declarado**, e isso é ambíguo: mouse e teclado declaram o mesmo canal
para o mesmo host. Corrigido fazendo o agente reportar o nome HID++
(feature 0x0005) e casando por ele.

A medição que fechou o argumento, no mesmo teste:

```
aqui:     slot 1 = MX Anywhere 3            slot 2 = MX Keys
Windows:  slot 1 = MX Keys                  slot 2 = MX Anywhere 3
```

**Invertido.** Slot é por receptor e não coincide — já suspeitávamos, e
agora está medido. Qualquer lógica baseada em número de slot entre
máquinas está errada por construção.

### Travas

- **Desligado por padrão.** Um reconciliador que erra te deixa sem teclado.
- **Nunca age sem saber onde o líder está.** Canal desconhecido = não mexe.
- **Mesmo mutex das outras trocas** (`focus::switch_lock`): borda, API e
  reconciliador nunca competem.
- **8 s de espera depois de agir**, para não brigar consigo mesmo.

## Velocidade: 105 s → 1,3 s, em quatro passos medidos

| | troca ponta a ponta |
|---|---|
| solaar | **105 s** |
| HID++ direto | 13,8 s |
| + paralelismo e cache de nomes | 3,9 s |
| + modo rápido (cache de pares) | **1,32 s** |

Cada passo saiu de uma medição, não de intuição:

**1. Paralelizar o que não depende.** A varredura local (1,6 s) e o probe
do agente por SSH (2,7 s) rodavam em sequência no `plan()`. Juntas em
`thread::scope`: 2,2 s em vez de 4,3 s.

**2. Não reler o que já se sabe.** O `survey` lia o nome de cada
dispositivo por HID++ a cada chamada — ~1,1 s dos 1,6 s gastos para
descobrir o que o cache já tinha.

**3. Varrer só os slots conhecidos.** Os 6 slots custavam ~1 s só em
timeout dos 4 vazios. Com o cache, varremos 2 — e uma varredura completa
a cada 10, senão um dispositivo pareado depois nunca apareceria.
`/api/devices`: 1,63 s → **0,36 s**.

**4. Não esperar SSH no caminho quente** (ideia do usuário). O probe do
agente era ~2,2 s dos 3,9 s. Um refresher em background
([`peers.rs`](../logid/src/peers.rs)) mantém o estado dos pares a cada
6 s, e `"fast": true` lê o cache:

```
dry-run normal  2,193 s
dry-run rapido  0,197 s     <- 11x
troca rapida    1,32 s
```

A checagem **declara a idade** do dado em vez de fingir frescor:
`agente responde (cache de 7220ms atras)`. A vigilância de borda usa o
modo rápido; a API sem `fast` continua sondando de verdade.

**5. `LEAVE_WAIT` de 300 ms para 120 ms.** O dispositivo sai quase
instantaneamente — a confirmação chega na primeira tentativa.

## Bateria por HID++, e um achado

Dois features, porque dispositivos da mesma família usam diferentes —
medido: o MX Anywhere 3 expõe **0x1004** (porcentagem) e o MX Keys
**0x1000** (faixa). Tentamos o primeiro e caímos para o segundo.

Bytes crus, para o parser não ser suposição:

```
slot 1  0x1004 getStatus   -> 05 01 00      (5%, nivel 1)
slot 2  0x1000 getLevel    -> 05 00 00      (5%, descarregando)
slot 2  0x1000 getCapability -> 04 04 c0 06 0a   (critico = 0x0a = 10%)
```

Validado contra fonte independente: o `upower`, que lê pelo driver
`hid-logitech-hidpp` do kernel, reporta **o mesmo** — 5%,
`warning-level: critical`. Os dois dispositivos estão quase sem carga, e
isso explica o "ping perdido" que eu havia atribuído a economia de
energia: um dispositivo a 5% demora a responder.

A bateria **não** entra no caminho quente (`survey_opts(false)`): custa
duas idas e voltas por dispositivo e não influencia decisão de troca.

## Remapeamento de botão (0x1B04)

Lido do seu hardware, e os nomes conferem com o que o Solaar exibia:

```
slot 1 (MX Anywhere 3) -- 7 controles
  0x0050 Botao esquerdo        flags=0x01   nao reprogramavel
  0x0051 Botao direito         flags=0x01   nao reprogramavel
  0x0052 Botao do meio         flags=0x31   reprogramavel + divertable
  0x0053 Voltar                flags=0x31
  0x0056 Avancar               flags=0x31
  0x00c4 Smart Shift           flags=0x31
  0x00d7 Botao de gesto        flags=0xa0

slot 2 (MX Keys) -- 24 controles (teclas F e midia)
```

Os flags vêm do próprio dispositivo: `0x10` = reprogramável, `0x20` =
divertable. Os botões esquerdo e direito reportam `0x01` — **sem** o bit
`0x10`, e o `set_control_remap` recusa com essa justificativa em vez de
tentar e falhar obscuro.

### API

| | |
|---|---|
| `GET /api/controls` | controles de cada dispositivo **online** |
| `POST /api/controls/remap` | `{slot, cid, to, confirm}`; `to: "0"` volta ao padrão |

Exige `confirm`, como as outras ações que mudam hardware. Recusas
testadas: sem `confirm` → 400, dispositivo em outro canal → 409, id
inválido → 400. Dispositivo ausente não aparece em `/api/controls` —
listar nome sem poder ler nem escrever seria enganoso.

### Escrita validada contra hardware

Ciclo completo, com os dispositivos parados:

```
POST remap 0x0056 -> 0x0052    {"detail":"0x0056 (Avancar) -> 0x0052 (Botao do meio)","ok":true}
GET  controls                   0x0056 Avancar -> 0x0052 (Botao do meio)
POST remap 0x0056 -> padrao     {"detail":"0x0056 (Avancar) -> padrao","ok":true}
GET  controls                   todos -> padrao
```

E o teste pegou dois bugs antes de o usuário sentir:

**`to = 0` não restaura o padrão.** A escrita retornava `ok` e o
dispositivo continuava reportando o remapeamento antigo. No 0x1B04 o
padrão se restaura remapeando o controle **para ele mesmo**. Agora
`to: "0"` é traduzido para o próprio `cid`.

Se eu tivesse confiado no retorno `ok` em vez de ler de volta, teria
deixado o botão Avançar do usuário alterado e reportado sucesso.

**O gate de ping precisava de retry.** O `remap` falhou com
`slot 1 nao responde` segundos depois de uma leitura bem-sucedida. Eu
havia posto retry no `survey` e esquecido no `remap` e no `change_host`
— com os dispositivos a 5% de bateria, um ping único perde. Os três
agora tentam de novo (3 a 4 vezes, 180 ms de intervalo).

### Limite observado: bateria baixa quebra enumeração longa

O teclado tem 24 controles, e a 5% ele não sustenta as ~50 idas e
voltas: `/api/controls` devolve
`MX Keys Wireless Keyboard (erro: sem resposta)` enquanto o mouse, com
7 controles, completa. O erro aparece por dispositivo em vez de
derrubar a resposta inteira.

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
