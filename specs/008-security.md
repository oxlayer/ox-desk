# 008 — Segurança e trust model

Tudo na LAN. Nenhuma conta, nenhuma nuvem, nenhuma telemetria.
Auditado nos binários que já usamos (ver README): Deskflow, Solaar e
GSConnect não têm endpoint de telemetria; o único capaz de falar com a
internet é o update check opt-in do Deskflow, que vem
`enableUpdateCheck=false`.

## Por que só localhost

O `logid` escuta em `127.0.0.1`, não em `0.0.0.0`. Ele **lê portas,
executa SSH e dispara troca de canal HID++** — é superfície de
privilégio, não um site. Quem alcançar a API alcança as duas máquinas.

Se um dia quiser abrir para o celular, não bind em `0.0.0.0`: exponha
por SSH tunnel ou por um reverse proxy com auth. A diferença é
intencional.

## Confiança entre as máquinas

Três pares de credencial, cada um da ferramenta dona. **Não
centralizar** — reinventar key management é como se perdem chaves.

| Canal | Credencial | Verificação |
|---|---|---|
| SSH | ed25519 **com passphrase**, no `gcr-ssh-agent` | fingerprint em `known_hosts`, conferida ao migrar IP→nome |
| Deskflow | TLS, cert em `~/.config/Deskflow/tls/` | fingerprint aprovado no primeiro contato (`trusted-servers`) |
| KDE Connect | TLS mútuo, cert por device | *verification key* `4BCF0AF8` |

Os três usam **trust on first use com confirmação humana**. Isso não é
atrito a eliminar: é a única defesa contra alguém na mesma LAN se passar
pela outra máquina. Um "pareamento de um clique" que aceita fingerprint
sozinho remove a proteção inteira.

**Regra:** o `logid` pode *mostrar* os fingerprints lado a lado para
facilitar a comparação. Nunca pode aceitar por você.

## Riscos ligados hoje, conscientemente

| Risco | Estado | Mitigação |
|---|---|---|
| RDP do GNOME na 3389, senha, toda a LAN | **ligado** | `grdctl rdp disable`; o painel deve mostrar isso como risco |
| Clipboard espelhando senha do Bitwarden | mitigado | `send-content=false` — unidirecional ([`004`](004-clipboard.md)) |
| Chave SSH com acesso admin no Windows | aceito | passphrase + agente de sessão |
| `hidraw` acessível ao usuário | necessário | `uaccess` por udev, só usuário logado localmente |
| Input trafegando na LAN (Deskflow) | aceito | TLS 1.3, fingerprint conferido |

## O que nunca gravar

- Passphrase, senha ou chave privada em `config.toml` ou no estado.
- Conteúdo de clipboard em log.
- Fingerprint aceito automaticamente, sem registro de quando e por quem.

## Modo de falha que é problema de segurança

Perder teclado e mouse nas duas máquinas (ver
[`005`](005-input-switch.md#rollback-não-é-opcional)) te deixa sem como
intervir numa máquina destravada. Rollback automático não é conveniência:
é controle de acesso.
