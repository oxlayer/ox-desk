# 012 — Alcançar as máquinas fora da LAN

> Estado: overlay instalado no Windows; Ubuntu e Mac pendentes (ver
> "O que falta"). O `--bind` e o `--mbps` estão prontos e medidos.

## O que muda em relação à spec 008

A [spec 008](008-security.md) diz "tudo na LAN, nenhuma conta, nenhuma
nuvem". Esta spec abre uma exceção estreita e diz o preço.

O que **não** muda: o `logid` continua escutando só em `127.0.0.1`. O
painel de fora chega por túnel SSH (`ssh -L 8787:127.0.0.1:8787 …`),
que é literalmente o que a 008 já mandava fazer. Nenhuma porta de
controle passa a escutar na rede.

O que muda: as três máquinas entram num overlay, e o MacBook passa a
alcançá-las de qualquer lugar.

## Por que overlay e não túnel HTTP

O transporte da tela é QUIC, que é **UDP** ([spec 010](010-remote-desktop.md)).
Isso elimina metade das opções antes de começar:

| Ferramenta | Carrega nosso UDP? | Caminho |
|---|---|---|
| Cloudflare Tunnel (`cloudflared`) | ❌ só TCP/HTTP | — |
| Cloudflare Mesh (ex-WARP Connector) | ✅ TCP/UDP/ICMP | **relay pela borda da Cloudflare** |
| Tailscale | ✅ | direto entre os pares; relay (DERP) só quando o NAT impede |
| WireGuard puro | ✅ | direto, mas **sem travessia de NAT**: exige endpoint alcançável |

Escolhido **Tailscale**. O que decidiu não foi a facilidade: foi o
caminho. Cada quadro de tela atravessa a rede 60 vezes por segundo, e o
Mesh põe a borda da Cloudflare no meio de todos eles. O Tailscale tenta
direto primeiro.

WireGuard puro ficaria mais fiel ao "nada de nuvem", mas precisaria de
endpoint público — port-forward com IP dinâmico, ou um VPS. Mais peças
para quebrar, e a coordenação do Tailscale só vê metadado de
conexão: o tráfego é WireGuard fim-a-fim entre as máquinas.

### Risco aceito

Uma conta de terceiro passa a ser parte do caminho: se o coordenador
do Tailscale estiver fora, máquina nova não entra e chave expirada não
renova. As sessões já estabelecidas seguem. É o mesmo tipo de
dependência que a 008 evitou até aqui, e entra como risco consciente,
não como esquecimento.

## A exposição que isto cria, e o que foi feito

O canal de tela **cifra e não autentica**. O cliente aceita qualquer
certificado (`AceitaQualquer`, em `logistream/src/net.rs`, marcado como
temporário desde que foi escrito) e o host nunca pediu nada do cliente.
Quem alcança a 9777 assiste; quem alcança a 9778 digita.

Enquanto as três máquinas ficavam em casa, "quem alcança" era a LAN de
casa. **O MacBook sair muda isso**: em `0.0.0.0`, num Wi-Fi de hotel,
ele serve tela e teclado para a rede inteira do hotel.

### Feito: `--bind`

```
logistream-host [porta] [tela] [--bind todas|overlay|<endereço>] [--mbps N]
```

- `todas` (padrão) — `0.0.0.0`, exatamente o de sempre. Nenhuma máquina
  muda de comportamento sem alguém pedir.
- `overlay` — só o endereço do overlay.
- `<endereço>` — à mão.

Duas decisões que o código impõe, cada uma com teste que quebra se
alguém "melhorar":

**`overlay` falha quando não acha o overlay.** Não cai para `0.0.0.0`.
A queda silenciosa seria confortável e seria o bug: a pessoa pediu
proteção, o Tailscale não subiu, e o host abriria para o hotel sem
ninguém saber.

**Faixa + nome da interface, nunca só a faixa.** 100.64.0.0/10 é a
CGNAT (RFC 6598) e o Tailscale usa ela — mas ela é CGNAT de verdade
também. Um celular compartilhando internet entrega endereço dessa faixa
para quem conecta, então o MacBook em hotspot (justamente o cenário de
teste) teria um `en0` em 100.x, e o critério ingênuo escutaria na rede
do celular achando que era o overlay.

### Pendente: pareamento por chave

O certo é o que a 008 já descreve para os outros três canais: trust on
first use com confirmação humana. O canal de tela é o único que viola a
regra do próprio projeto. `--bind` fecha a porta; não troca a fechadura.

## Banda: `--mbps`

O alvo de bitrate escala com a área (~8 Mbps em 1080p, ~15 em 1440p,
~28 na Retina). A fórmula estava **duplicada** — `nvenc.rs` e, à mão,
`host_mac.rs` — e agora mora em `qualidade.rs`, com teste.

**Medido:** o upload de casa entrega ~136 Mbps (25 MB em 1,46 s contra
speed.cloudflare.com). Ou seja, **o gargalo não é daqui**: 15 Mbps
saem sem esforço. Quem aperta é o outro lado — Wi-Fi de hotel costuma
dar 10–50 Mbps de download, variável, com fila.

Por isso o teto existe e por isso ele é escolhido para a sessão
remota, não gravado na máquina.

### O que o teto não é

Não é controle de congestionamento. Escolhido uma vez, não se mexe. Se
o link piorar no meio da sessão, nada reage.

O passo seguinte é fechar o laço, e o sinal já existe de graça: o
`quinn` mede RTT, perda e eventos de congestionamento do caminho
(`Connection::stats()`), e o host já conta `recusados` quando o
controle de congestionamento recusa um datagrama. Nada consome nenhum
dos dois. Realimentar o encoder com isso é, em uma frase, o que o BUD
da Parsec faz de diferente do nosso.

## mDNS não morre, e isso foi medido

Preocupação levantada e **descartada por medição**: nesta máquina os
nomes `.local` não passam pelo systemd-resolved.

```
hosts: files mdns4_minimal [NOTFOUND=return] dns
```

O Avahi responde pelo NSS **antes** do `dns`, então qualquer resolvedor
que o Tailscale configure nem é consultado para `.local`. O
`resolvectl` confirma: mDNS está desligado em todos os links, e
`getent hosts windows-host.local` resolve assim mesmo.

Se um dia quebrar: `sudo tailscale set --accept-dns=false`.

## Receita

```bash
# Ubuntu (precisa de sudo -- o daemon não instala pacote sozinho)
sudo bash scripts/instalar-tailscale.sh
sudo tailscale up

# Windows (feito: MSI com TS_UNATTENDEDMODE=always)
#   `always` mantém o túnel de pé SEM usuário logado -- a máquina
#   continua alcançável na tela de bloqueio, que é o estado em que ela
#   vai estar quando eu quiser alcançá-la de fora.

# conferir, dos DOIS lados -- "direto" é propriedade do par
bash scripts/conferir-overlay.sh
```

## O teste que vale

Medir "fora da LAN" de dentro de casa não prova nada. O único teste
honesto é **prender o MacBook num hotspot de celular** (desligando o
Wi-Fi de casa) e abrir uma tela de verdade. O que importa medir lá:

- `tailscale ping` diz `direct` ou `via DERP`? Relay = CGNAT dos dois
  lados; funciona, mas a tela sente.
- fps e quadros perdidos no `logistream-sink`.
- latência de ponteiro, que é o que a pessoa percebe primeiro.

## O que falta

- Ubuntu: instalar e logar (precisa do sudo do usuário).
- Windows: `tailscale up` — instalado e esperando login.
- Mac: fora do ar o dia inteiro; falta instalar, logar, e **só então**
  passar o host dele para `--bind overlay`. Flipar antes de o overlay
  existir lá deixaria o Mac sem tela remota nem em casa.
- Adaptação de bitrate pelo `quinn::Connection::stats()`.
- Pareamento por chave pinada no canal de tela.

## Veja também

- [[008-security]] — o trust model que esta spec abre com cuidado
- [[010-remote-desktop]] — por que QUIC, e por que isso restringe o overlay
- [[../.claude/skills/configurar-maquina]] — o wizard, que já mede SSH entre pares
