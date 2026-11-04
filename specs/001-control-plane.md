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

### O que a simetria ainda exige

`focus::plan` **bloqueia** a troca quando o alvo não tem `ssh` + `agent`,
porque sem alguém do outro lado capaz de devolver o dispositivo a origem
não pode desfazer ([`005`](005-input-switch.md)). O `watch-edge`
escapava disso por não passar pelo `focus`.

Então para o Windows virar origem de verdade faltam duas peças, e
nenhuma delas é afrouxar o check:

1. O `logid` ganhar os subcomandos de CLI que o `agent` tem (`devices`,
   `host-info`, `set-host`), para o Ubuntu ser sondável do mesmo jeito.
2. `sshd` no Ubuntu — hoje `inactive`, nada escutando na 22 — e uma
   chave do Windows para cá.

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
