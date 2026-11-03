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
