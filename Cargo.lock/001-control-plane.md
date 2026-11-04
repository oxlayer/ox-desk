# 001 — Control plane (`logid`)

Implementado em Rust em [`logid/`](../logid/). O protótipo Python
(`monitor.py`) foi substituído na fase 1 — ver
[`000`](000-overview.md#linguagem-rust).

## Simétrico: todo nó é par, nenhum é central

**Decisão de 03/11, do usuário:** com o daemon rodando, *qualquer*
máquina pode virar host; quem manda é onde a pessoa está sentada.

A primeira versão desta spec dizia "no Ubuntu roda como control plane,
no Windows como agente". Isso estava errado por dois motivos:

1. **O hardware já impõe simetria.** Só quem detém o dispositivo pode
   emitir `ChangeHost` ([`005`](005-input-switch.md#restrição-descoberta-só-quem-detém-o-device-pode-empurrá-lo)).
   Um nó central que não tem o device não consegue mover nada — ele só
   pode *pedir*.
2. **São três máquinas**, não duas: Ubuntu, Windows e um MacBook no
   canal 3. Eleger uma como central é arbitrário.

### O modelo

Todo nó roda o **mesmo** `logid`. Nenhum papel fixo. O estado que
importa é derivado, não configurado:

| Pergunta | Quem responde |
|---|---|
| quem detém os dispositivos? | cada nó lê o próprio receptor; exatamente um vê `online=true` |
| quem é o host atual? | o detentor |
| quem pode trocar? | **só o detentor** |

Pedir foco é portanto um **encaminhamento**, não um comando:

```
eu (qualquer nó)  --pede-->  detentor atual  --ChangeHost-->  alvo
```

Se eu já sou o detentor, o encaminhamento é local. Se eu sou o alvo,
estou pedindo que me entreguem. Se não sou nenhum dos dois, sou só o
intermediário — e isso é legítimo: a GUI de qualquer máquina deve poder
mover o foco entre as outras duas.

### Transporte entre pares: SSH

Continua SSH, por simetria e porque já funciona
([`002`](002-ssh.md)): nenhuma porta nova, nenhuma auth nova, e o
macOS já traz sshd. **Pendência:** o Ubuntu ainda não tem sshd — hoje
só ele disca para fora. Para a simetria valer, `openssh-server` aqui
entra no bootstrap.

> Alternativa descartada: protocolo próprio na LAN com mTLS. Seria
> reimplementar o que o SSH já entrega, e multiplicar superfície — ver
> o critério em [`000`](000-overview.md#o-que-construímos-e-o-que-integramos).

### O que deixa de existir

- O conceito de "agente" como binário separado. O
  [`agent/`](../agent/) atual é um passo intermediário, não o destino:
  o alvo é **um binário por plataforma, mesmo papel**.
- `[[hosts]]` com um nó marcado como local por ter `addr = 127.0.0.1`.
  Cada nó sabe quem é por hostname; os outros são pares.

## O mesmo binário nos dois SOs

Esta spec afirmava "todo nó roda o **mesmo** `logid`" desde 03/11. Era
aspiração: o daemon compilava só no Linux. O Windows tinha o
`logid-agent`, um CLI de um disparo, sem API, sem bandeja, sem janela — e
a GUI do Ubuntu alcançava a outra máquina por SSH, chamando o CLI na mão.

O usuário notou pelo buraco que isso deixa: *"por que não tem a GUI e
atalho na taskbar no Windows, só aqui no Ubuntu?"*. A resposta é que a
**troca** ficou simétrica (o `watch-edge` deu a borda ao Windows) mas o
**control plane** não. Quem estivesse sentado no Windows não tinha painel.

### O que de fato amarrava ao Linux

Medido antes de decidir, módulo por módulo. Dos nove, **seis são
portáveis como estão** (`agent`, `config`, `focus`, `follow`, `hidpp`,
`peers`). Cinco pontos amarravam:

| ponto | Linux | Windows |
|---|---|---|
| fallback de DNS | `getent hosts` | nada — o resolver já fala mDNS |
| estado de `units` | `systemctl --user is-active` | serviço, senão processo |
| hora local | `date +%z` + aritmética | `GetLocalTime` |
| origem dos logs | `journalctl --user -u` | arquivo em `%LOCALAPPDATA%` |
| acesso ao HID | ACL de `uaccess` em `/dev/hidraw` | abrir o receptor |
| detecção de borda | barreira do `Mutter.InputCapture` | `GetCursorPos` |
| diretório de config | `~/.config/logi` | `%APPDATA%\logi` |

Daí [`logid/src/sys/`](../logid/src/sys/): um lugar para o que divergir,
em vez de um segundo daemon no `agent/` duplicando os outros seis
módulos. A alternativa — crescer o agente até virar daemon — custaria
duas cópias de `focus`, `follow` e `peers` que precisariam concordar.

### A permanência na borda se expressa diferente nas duas pontas

Não é inconsistência, é o sinal disponível. O mutter entrega eventos
discretos, então no Linux a permanência é **densidade**: `hold_hits`
ativações dentro de `hold_window_ms`. No Windows medimos contato
**contínuo**, que é o que o polling a 30 ms de fato vê. A tradução:
`hold_hits <= 1` troca no primeiro toque, acima disso exige ficar na
borda por `hold_window_ms`.

### O `watch-edge` virou thread, e reivindica o mesmo mutex

Com o daemon no Windows, o processo separado do `watch-edge` deixa de
fazer sentido — e passa a ser *perigoso*: dois donos do mesmo receptor
HID++ é exatamente o que a trava de instância única existe para impedir
(já aconteceu, três `watch-edge` simultâneos triplicando o tráfego de
rádio).

Por isso [`border_win.rs`](../logid/src/border_win.rs) reivindica o
**mesmo nome de mutex** do `watch-edge`, `logid-agent-watch-edge`. Não é
coincidência: é o que garante que o atalho antigo e o daemon novo nunca
pollem juntos, mesmo que alguém esqueça de remover o atalho do
`shell:startup`.

A thread da borda também é a única que desiste fora da sessão do console
— `GetCursorPos` e `GetLastInputInfo` são por sessão, e na sessão 0 (o
caso de lançar por SSH) ela leria um desktop falso de 1024x768 e um idle
igual ao uptime. O resto do daemon funciona lá; só a borda não.

### Quem sou eu passou a precisar de resposta

Era implícito — `addr = "127.0.0.1"` e um texto livre em `role` dizendo
"este PC". O dead-man's switch do Windows precisa saber para onde
devolver o dispositivo, e "o host que não sou eu" exige saber quem eu
sou. Daí `local` em `[[hosts]]`, opcional, com a heurística de loopback
como fallback para as configs que já existem.

Ao escrever isso apareceu uma segunda leitura do mesmo critério: o
`follow.rs` tinha a **própria** cópia de `is_local`, que ignorava a flag
nova. Num Windows com `local = true` e endereço não-loopback, o `follow`
e a borda discordariam sobre qual máquina é "eu". A cópia foi removida e
`quem_sou_eu_por_loopback_ou_por_flag` é o teste que impede ela de
voltar.

### Como ele sobe no Windows

Não é serviço. Um serviço roda na sessão 0, onde `GetCursorPos` e
`GetLastInputInfo` não veem nada — e a borda vai precisar da sessão do
console. É **tarefa agendada** com `/IT` (*interativo apenas*), que roda
na sessão do usuário no login e não pede senha guardada:

```powershell
schtasks /create /tn logid /tr "%LOCALAPPDATA%\logid\logid.exe" /sc onlogon /it /f
```

Verificado: `pid 2356 sessão 1`, escutando na 8787.

Antes disso tentei `Start-Process` por SSH. Subiu, respondeu `healthz` —
e morreu junto com a sessão SSH, porque o OpenSSH do Windows mata a
árvore de processos ao desconectar. É o mesmo princípio da regra
`long-running-jobs` do outro repo: trabalho residente não vive pendurado
numa sessão que vai embora.

O binário é `windows_subsystem = "windows"`: sem isso uma janela de
console aparece a cada login e a cada vez que a GUI sobe o daemon. Não
afeta o JSON por SSH — o subsistema decide se um console é *alocado*, não
para onde o stdout vai; com um pipe do pai, a saída continua chegando
(verificado com `Start-Process -RedirectStandardOutput`).

A GUI é o mesmo Electron do Ubuntu, em `%LOCALAPPDATA%\logid\gui`, com
dois atalhos — Menu Iniciar (fixável na taskbar) e `Startup` — apontando
para `electron.exe .` com `icon.ico` multi-resolução. Os caminhos que
mudam por SO estão em `paths()` em [`gui/main.js`](../gui/main.js), e têm
de bater com `sys::config_dir`: divergir ali significa a GUI ler outra
config e não achar o daemon.

> O `npm install` **não** baixou o binário do Electron (13 pacotes em 5 s,
> sem `dist/`), apesar de `ignore-scripts = false`. `node
> node_modules/electron/install.js` na mão resolveu. Se a GUI de lá
> parar de abrir depois de um `npm ci`, é o primeiro lugar a olhar.

### A isolação de sessão também engana quem verifica

Gastei várias rodadas convencido de que a janela da GUI não abria:
`MainWindowHandle` vinha `0` para os quatro processos do Electron, e meu
`EnumWindows` não achava janela nenhuma. A conclusão estava errada.

`EnumWindows` só enumera janelas da **window station da sessão de quem
chama**, e meu SSH está na sessão 0. `MainWindowHandle`, do .NET, usa o
mesmo mecanismo. Não era a janela que não existia — era eu olhando do
lugar errado, exatamente a armadilha que esta spec já documentava para o
`GetCursorPos`, aplicada agora à *verificação*.

O que de fato provou que a GUI funciona foi o stderr do renderer: um
aviso de CSP vindo da **nossa** página, o que só acontece depois de ela
carregar. E o aviso era achado real — o painel não declarava
Content-Security-Policy, o que equivale a permitir `unsafe-eval`. A
página não usa `eval` nenhum (conferido), então a política foi declarada
e o aviso sumiu.

Lição operacional: ao verificar algo da sessão interativa a partir de uma
sessão não interativa, prefira sinais que atravessam a fronteira —
arquivo de log, porta TCP, processo — a APIs de janela, que não
atravessam.

### A simetria fechou (03/11)

`focus::plan` **bloqueia** a troca quando o alvo não tem `ssh` + `agent`,
porque sem alguém do outro lado capaz de devolver o dispositivo a origem
não pode desfazer ([`005`](005-input-switch.md)). O `watch-edge`
escapava disso por não passar pelo `focus` — funcionava, mas sem os gates.

Faltavam duas peças, e nenhuma era afrouxar o check:

1. **O `logid` virou também o "agente" do próprio host.** `logid devices`,
   `host-info`, `set-host` respondem uma linha de JSON, igual ao
   `logid-agent`. O formato vive em
   [`hidpp/src/protocol.rs`](../hidpp/src/protocol.rs), compartilhado
   pelos dois binários: é o contrato *entre as máquinas*, o pior lugar
   possível para duas cópias. (Era `survey()` dentro do agente; a segunda
   cópia ia divergir e aparecer como "agente nao respondeu JSON" num log,
   longe da causa.)
2. **`sshd` no Ubuntu**, por [`scripts/enable-sshd.sh`](../scripts/enable-sshd.sh),
   só por chave e só para um usuário.

Verificado no ensaio (`POST /api/focus` com `confirm: false`) **do lado
Windows**:

```
[ok  ] dead-man's switch no alvo
       agente responde; dispositivos la: slot 1 (canal 1/3), slot 2 (canal 1/3)
[BLOQ] posse do dispositivo
       'MX Anywhere 3' esta offline aqui -- ja esta em outro canal
```

Os bloqueios restantes são os **certos**: só quem detém o dispositivo pode
empurrá-lo, e naquele momento ele estava no Ubuntu.

Com isso o `watch-edge` foi aposentado — atalho removido do
`shell:startup`, processos encerrados — e `[input.border]` ligada no
Windows. O reconciliador (`[input.follow]`) ficou **desligado** lá de
propósito: é um reconciliador, não dois. O do Ubuntu já cobre as duas
máquinas, agindo direto quando o dispositivo está lá e pedindo por SSH
quando está aqui; ligar ambos faria os dois chegarem à mesma conclusão e
agirem em dobro.

### O host precisa declarar o SO

Dois lugares montam comando para rodar **no par**, e os dois eram
específicos de Windows: `agent::devices` monta PowerShell (`& "exe"
devices`) e `probe::ssh_detail` roda um one-liner com CIM. Contra Ubuntu
ou macOS os dois falhariam — e falhariam **em silêncio**, porque o
detalhe é enfeite e o probe do agente já degrada para "não respondeu".

Daí `os = "windows" | "linux" | "macos"` em `[[hosts]]`, com inferência
para as configs que já existiam. No Unix o comando vai com aspas simples
e sem o `&` — que ali significaria *background* e devolveria sucesso
vazio, lido como "agente nao respondeu JSON".

## API

Prefixo `/api`. JSON em tudo. Sempre em `127.0.0.1` (ver
[`008`](008-security.md#por-que-só-localhost)).

| Método | Rota | Função |
|---|---|---|
| `GET` | `/healthz` | vivo? **Não** toca em rede — existe porque o health check de 800 ms batia no `/api/status`, que leva 2 s. Erro real já cometido. |
| `GET` | `/api/hosts` | máquinas, estado, serviços |
| `GET` | `/api/hosts/{id}` | detalhe (uptime, RAM, dispositivos HID) |
| `POST` | `/api/hosts/{id}/focus` | **trocar o foco para esta máquina** — ver [`005`](005-input-switch.md) |
| `GET` | `/api/config` | config efetiva + de onde veio cada valor |
| `PATCH` | `/api/config` | alterar; valida antes de gravar |
| `POST` | `/api/bootstrap/{id}` | provisionar a outra máquina via SSH ([`002`](002-ssh.md)) |
| `GET` | `/api/files/{id}?path=` | listar diretório ([`003`](003-files.md)) |
| `GET` | `/api/clipboard` / `POST` | ler/empurrar clipboard ([`004`](004-clipboard.md)) |
| `GET` | `/api/events` | SSE: mudanças de estado, para a GUI não ficar em polling |

## Estado

Dois níveis, separados de propósito:

- **Config** — o que você escolheu. Arquivo versionável, legível, 
  editável à mão: `~/.config/logi/config.toml`. Nunca escrevemos
  segredo aqui.
- **Runtime** — o que foi descoberto. Cache descartável em
  `~/.local/state/logi/`. Apagar não perde nada além de uma releitura.

Segredo (chave SSH, cert TLS) **não** entra em nenhum dos dois: fica
onde a ferramenta dona já guarda (`~/.ssh`, `~/.config/Deskflow/tls`) e
referenciamos por caminho.

## Adapters

Cada integração é um módulo com a mesma forma: `probe()`, `read()`,
`apply()`. Nada de ferramenta externa chamada direto de um handler —
mesma razão da regra de [funções puras](../CLAUDE.md) no outro repo: o
handler resolve IDs, o adapter faz I/O, a decisão é função pura
testável.

| Adapter | Envolve | Notas |
|---|---|---|
| `ssh` | OpenSSH client | chave dedicada, `BatchMode`, agente para a passphrase |
| `sftp` | OpenSSH | file browser; sem dependência nova |
| `deskflow` | `deskflow-core`, `Deskflow.conf` | **servidor segfalha no Linux** — só modo cliente |
| `hidpp` | Solaar / HID++ 0x1814 | troca de canal Easy-Switch |
| `kdeconnect` | GSConnect DBus / `dconf` | clipboard, share, notificações |
| `rustdesk` | binário RustDesk | tela; só ciclo de vida, não protocolo |

## GUI

A casca Electron em [`gui/`](../gui/) já faz
janela, bandeja, instância única e reaproveita o backend do systemd.
Ela **não duplica lógica**: toda regra vive no `logid`.

Armadilha registrada: o host de extensões do VS Code exporta
`ELECTRON_RUN_AS_NODE=1`, e com ela o binário do Electron roda como Node
puro e nada abre. `run.sh` e `npm start` limpam a variável.

## Princípios

1. **Idempotente.** Toda rota de `apply` pode ser chamada duas vezes.
2. **Diga de onde veio o valor.** `/api/config` devolve origem junto
   com o valor — default, arquivo, ou descoberto.
3. **Nunca escrever IP.** Ver [`007`](007-device-routing.md) e a seção
   *"O IP muda"* do README.
4. **Falhar barulhento.** O `langfuse` do outro repo ensinou: `catch`
   que engole erro custa horas. Se um adapter não achou a ferramenta,
   isso aparece na API e na GUI.
