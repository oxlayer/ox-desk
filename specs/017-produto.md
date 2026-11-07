# 017 — De infra pessoal a produto

> Estado: decidido em 07/11. Fase A do pareamento em andamento.

## A decisão

O ox-desk deixa de ser só a infra de três computadores e vira um produto de
acesso remoto, na categoria do Parsec e do AnyDesk:

- **open source**, e qualquer pessoa pode hospedar o próprio servidor;
- **ou usa os nossos servidores**, que é a versão paga;
- **telemetria básica, que pode ser desligada**, para medir o que decide o
  produto: quantas conexões saem diretas e quantas caem no relay.

Isso muda duas premissas antigas, e esta spec é o registro dessas mudanças:

| Antes | Agora |
|---|---|
| "Nada de telemetria, nada de nuvem" (`CLAUDE.md`, spec 008) | Telemetria mínima, ligada por padrão, que pode ser desligada. O servidor é opcional e pode ser hospedado por qualquer pessoa |
| Confiança vinda da LAN e do overlay (spec 012) | Confiança vinda da **chave de cada máquina**. A rede, inclusive o nosso servidor, nunca precisa de confiança |

## Duas regras que não se perdem

**Só com LAN continua funcionando.** Sem internet, sem servidor e sem
telemetria, as máquinas da mesma rede se acham por mDNS e conectam direto,
com o mesmo pareamento por chave. O servidor de encontro só entra quando o
direto pela LAN não existe. Nenhum recurso local pode passar a depender
dele: perder o servidor tira o acesso de fora, e nunca o de dentro de casa.

**Todo computador é host e cliente.** É uma instalação só, com uma
identidade só, que serve a própria tela e abre a dos outros. A lista de
quem pode entrar (`autorizados`) é de cada máquina, e autorizar A a ver B
não autoriza B a ver A.

## A ordem

1. **Pareamento por chave.** Ele vale em todos os cenários, inclusive no
   Tailscale de hoje.
2. **Telemetria de caminho**, para medir quanto cai em relay: em casa, no 4G
   e fora.
3. **Um binário de encontro e relay** (`logi-encontro`), que nós hospedamos
   e que outras pessoas também podem hospedar.
4. **Cloudflare só como opção de hospedagem**, e nunca como dependência.
5. **Desligar o Tailscale**, só depois de o caminho próprio rodar sozinho por
   um tempo.

## 1. Pareamento por chave

### O que existe hoje

- O host gera um certificado novo a cada partida.
- O cliente aceita qualquer certificado (`AceitaQualquer`, em `net.rs`).
- A assinatura TLS do servidor também não é conferida.
- O host não pergunta quem é o cliente.

Na prática, o canal cifra e não autentica. Quem alcança a porta 9777 vê a
tela, e quem alcança a 9778 digita nela.

### O desenho

- **Toda instalação tem uma identidade:** um certificado com a chave
  (ECDSA P-256), gerado uma vez e guardado com permissão `600`. A digital é o
  SHA-256 do certificado.
- **O host apresenta essa identidade**, e não um certificado efêmero.
- **O cliente confere a digital do host** quando a recebe (`LOGI_PAR`). A
  assinatura TLS passa a ser verificada de verdade. Sem essa conferência,
  qualquer um poderia apresentar o certificado do host, que é público, sem ter
  a chave.
- **O cliente também apresenta a identidade dele.** O host confere essa
  identidade contra a lista de quem ele autoriza
  (`<config>/identidade/autorizados`, uma digital por linha).

### De onde vem a digital

- **Hoje, entre as nossas máquinas:** pelo SSH, que já é autenticado pelo
  `known_hosts`. O `logid` lê a digital do outro lado e a fixa. É um canal
  autenticado, e não confiança no primeiro uso.
- **No produto:** pelo código de pareamento. A pessoa lê um código curto nas
  duas telas e confirma, como na spec 008: o `logid` pode mostrar os códigos
  lado a lado, mas nunca pode aceitar sozinho.

### As fases (o servidor vai primeiro)

O host é quem aceita, então ele sobe antes. A recusa só liga depois de um
período de observação, como manda a lição do `observe` → `enforce`.

| Fase | Host | Cliente |
|---|---|---|
| **A, observar** | Apresenta a identidade persistente. Pede o certificado do cliente, sem exigir, e registra no log `cliente <digital> autorizado/desconhecido/sem certificado` | Apresenta a identidade. Confere a digital do host quando há `LOGI_PAR` e recusa se ela não bater. Sem `LOGI_PAR`, segue como hoje e avisa no log |
| **B, fixar** | Igual à fase A | O `logid` passa a digital pelo SSH. Sem digital, o painel não abre a sessão |
| **C, exigir** | Recusa cliente sem certificado e cliente fora da lista | Igual à fase B |

A virada para a fase C só acontece depois de o log da fase A mostrar zero
`desconhecido` e zero `sem certificado` nas conexões legítimas das três
máquinas.

## 2. Telemetria

### O que se manda

Um evento por sessão, no fim dela:

| Campo | Exemplo |
|---|---|
| versão, sistema do host e do cliente | `0.3.0`, `windows`, `macos` |
| caminho | `lan`, `overlay-direto`, `overlay-relay`, `nosso-direto`, `nosso-relay` |
| tipo de NAT, quando houver medida | `cone`, `simetrico` |
| RTT p50 e p95, fps, Mbps médio, quadros perdidos | |
| duração, em minutos arredondados | |
| motivo do fim | `fechou`, `caiu`, `recusado` |

### O que nunca se manda

- IP, nome de máquina, nome de usuário, caminho de arquivo ou conteúdo de
  tela, de teclado ou de clipboard;
- a digital da identidade. O id da instalação é um aleatório, separado dela,
  e muda quando a telemetria é desligada e ligada de novo.

### Como desligar

- `telemetria = false` no `config.toml`, ou o interruptor no painel;
- `LOGI_TELEMETRIA=0` no ambiente, que vence a configuração;
- quem hospeda o próprio servidor escolhe para onde vai a telemetria, ou a
  desliga para todos os clientes daquele servidor.

O painel mostra, antes do primeiro envio, o que é enviado, e mantém o último
evento visível para a pessoa conferir.

### Enquanto não há servidor

O evento é gravado num arquivo local (`<estado>/telemetria.jsonl`). Isso já
responde à pergunta do caminho com o Tailscale: o `tailscale status --json`
diz se o par está direto ou em relay (DERP).

## 3. Encontro e relay

Um binário só, `logi-encontro`, e uma imagem Docker:

- **Encontro:** cada máquina mantém uma conexão com ele e anuncia os
  endereços que vê (o local e o público, medido como STUN). Quando alguém
  quer conectar, o servidor apresenta um ao outro, e os dois tentam a
  conexão direta (*hole punching*) pelo mesmo socket UDP do QUIC.
- **Relay:** se o direto não sair em ~2 s, o tráfego vai pelo servidor. Como
  é QUIC cifrado de ponta a ponta com a identidade das máquinas, o relay
  repassa bytes que não consegue ler.
- O servidor sabe **quem fala com quem e quando**. Nunca vê a tela, o teclado
  ou os arquivos.

Referências de desenho: o hbbs/hbbr do RustDesk e o DERP do Tailscale.

### Onde hospedar e quanto custa

O custo é a banda do relay. Sinalização é desprezível.

| Opção | Banda | Observação |
|---|---|---|
| VPS em São Paulo | alguns TB incluídos | Barato, mas nós operamos |
| Cloudflare (TURN do Realtime) | ~1 TB grátis/mês, depois ~US$ 0,05/GB | Sem operação; preço a conferir antes de decidir |
| AWS sa-east-1 | ~US$ 0,15/GB | Caro para vídeo |
| Hetzner, DigitalOcean | banda barata | Sem região no Brasil: soma latência |

O binário não pode depender da Cloudflare. Ela entra, se entrar, como mais
um lugar para rodar o relay.

## Planos

A estrutura segue a do Parsec, que é a referência de mercado: grátis para
uso pessoal, e o pago vende o que custa para nós (relay, gestão de equipe,
auditoria). Os preços do Parsec em 07/11, por usuário e por mês: US$ 9,99
(individual), US$ 35 (times) e US$ 45 (empresa, só anual).

| Plano | O que tem |
|---|---|
| **Grátis** | Tudo que roda entre as máquinas da pessoa: LAN, conexão direta pela internet pelo nosso encontro, servidor próprio sem limite. Relay nosso com limite de banda |
| **Individual** | Relay sem limite, vários monitores, monitor virtual, cor 4:4:4, modo privado (tela física apagada) |
| **Times** | Computadores da equipe num lugar só, permissões por pessoa e por máquina, login único (SAML/OIDC), registro de acessos de 7 dias |
| **Empresa** | Relay dedicado ou na região do cliente, registro completo de acessos, papéis de administrador, API para automação |

O que fica grátis é o que não custa para nós. Hospedar o próprio servidor é
sempre grátis, e é o que mantém o projeto open source de verdade.

## O que esta spec não decide

- O preço em reais, e o que entra no limite do relay grátis.
- Conta de usuário e lista de máquinas na nuvem: começa sem conta, só com
  id e pareamento.
- O endpoint que recebe a telemetria (vem com o `logi-encontro`).

## Veja também

- [[008-security]] — o trust model que esta spec muda
- [[012-fora-da-lan]] — por que overlay, e o que falta
- [[016-sessao-e-visual]] — a sessão em aba, que passa a levar a digital
