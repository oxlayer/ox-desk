# 017 — De infra pessoal a produto

> Estado: decidido em 07/11. Fase A do pareamento no ar desde 07/11 (#11,
> build #166 nas três máquinas).

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

## O nome: oxdesk

"logi" e "logistream" vêm de Logitech, do tempo em que o projeto era trocar
teclado e mouse entre PCs. No produto, o Logitech é **um recurso** (a troca
de canal Easy-Switch para quem tem teclado e mouse deles e vários PCs), e o
nome passa a ser **oxdesk**.

| Hoje | Depois |
|---|---|
| `logid` | `oxdesk` (daemon e CLI) |
| `logistream-host`, `-view`, `-ponte`, `-kvm` | `oxdesk-host`, `-view`, `-ponte`, `-kvm` |
| `logi-ota` | `oxdesk-ota` |
| `~/.config/logi`, `%APPDATA%\logi` | `~/.config/oxdesk`, `%APPDATA%\oxdesk` |
| `%LOCALAPPDATA%\logid` | `%LOCALAPPDATA%\oxdesk` |
| `ai.logi.*` (launchd), tarefa `logid`, `logid.service` | `app.oxdesk.*`, tarefa `oxdesk`, `oxdesk.service` |
| `LOGI_*` (`LOGI_PAR`, `LOGI_PONTE_TOKEN`...) | `OXDESK_*` em inglês (`OXDESK_PIN`, `OXDESK_BRIDGE_TOKEN`...), aceitando o antigo por um tempo |
| crate `hidpp` | fica: é o protocolo da Logitech, e o nome é dele |

É uma troca de dois lados, e tem três armadilhas medidas antes:
- **O OTA instalado só conhece os nomes antigos.** Uma versão de transição
  tem de saber instalar a nova e trocar o próprio supervisor. Sem ela, a
  máquina que atualiza sozinha (Windows, Mac) para na versão antiga.
- **No Mac, a permissão de Gravação de Tela e de Acessibilidade é do
  binário.** Com o nome novo, a pessoa concede de novo, à mão.
- **No Windows, a exclusão do Defender é do caminho** `%LOCALAPPDATA%\logid`.
  A pasta nova precisa da exclusão antes de o binário chegar, ou ele é
  apagado na escrita (spec 008).

**E tudo que não é texto de tela fica em inglês:** nome de arquivo, script,
binário, módulo, identificador, variável de ambiente, chave de config, rótulo
de launchd e de tarefa. Português só no que a pessoa lê na tela, nos logs que
ela lê e nas specs. Entra no mesmo PR da troca de nome: `assinar.sh` →
`sign.sh`, `construir-binarios.sh` → `build-binaries.sh`, `ponte.rs` →
`bridge.rs`, `qualidade.rs` → `quality.rs`, `setup_comum.rs` →
`setup_common.rs`, `instalar-tela-virtual.ps1` → `install-virtual-display.ps1`,
e assim por diante. O que é chamado de fora (deploy, OTA, tarefas, launchd)
muda junto, e a busca pelo nome antigo tem de dar zero no fim.

A config e a identidade migram na primeira partida: se a pasta nova não
existe e a antiga existe, ela é movida, e o log diz isso.

Vai num PR próprio, depois de a fase A do pareamento estar no ar.

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
| **Grátis** | O núcleo inteiro, como no Parsec: 60 fps com baixa latência, teclado, mouse e controle, conexão ponta a ponta cifrada, host em Windows, macOS e Linux, compartilhar a tela por link. Mais: LAN, conexão direta pelo nosso encontro e servidor próprio sem limite. Relay nosso com limite de banda |
| **Individual** | Relay sem limite, vários monitores, monitor virtual, cor 4:4:4, modo privado (tela física apagada) |
| **Times** | Computadores da equipe num lugar só, permissões por pessoa e por máquina, login único (SAML/OIDC), registro de acessos de 7 dias |
| **Empresa** | Relay dedicado ou na região do cliente, registro completo de acessos, papéis de administrador, API para automação |

Do núcleo, ainda não temos: controle (gamepad) e compartilhar por link,
que é um convite de uso único, com prazo, que dá acesso a uma máquina sem
pareamento permanente.

O que fica grátis é o que não custa para nós. Hospedar o próprio servidor é
sempre grátis, e é o que mantém o projeto open source de verdade.

### O que mais o Parsec faz, e como fica aqui

Lido em parsec.app/pricing, em 07/11:

| No Parsec | Aqui |
|---|---|
| Grátis só para uso pessoal; trabalho exige licença | O código é MIT e não pode ter essa restrição. Ela vale só para **os nossos servidores**, pelos termos de uso. Quem hospeda o próprio servidor usa para o que quiser |
| Teste de 14 dias de Times e Empresa | Igual |
| Mensal, ou anual com desconto; Empresa só anual; sem desconto por volume | Igual, para começar |
| Colaboração por link: o outro observa ou assume o controle | O convite por link do grátis, com dois modos: só ver, ou ver e controlar |
| Acesso de convidado (Times): créditos comprados à parte, até 5 dias | Convite com prazo e sem pareamento permanente. Créditos à parte, no Times |
| Computadores da equipe, atribuídos a uma pessoa ou a um grupo | É o "Times" da tabela acima |
| No Times, membro só conecta em membro | Lista de autorizados vinda da equipe, e não de cada máquina |
| Bloquear que máquinas da rede da empresa sirvam a tela | Política da equipe que o host obedece |
| Relay dentro do firewall do cliente (Empresa) | O `logi-encontro` hospedado pelo cliente, que é o mesmo binário open source |

## O que aproveitar dos repositórios abertos do Parsec (lidos em 07/11)

Clonados e só lidos, sem compilar nem rodar:

| Repositório | Licença | Para nós |
|---|---|---|
| `libmatoya` (C, ativo) | MIT: dá para portar, com o aviso de copyright | Controle em `src/hid/` (PS4, PS5, Switch, Xbox, vibração); áudio no cliente (`windows/audio.c`, `resample.c`); cursor RGBA desenhado no cliente, gancho de Alt+Tab e tecla Windows, `keyboard.lock` e pointer lock (`windows/appw.c`, `unix/web/matoya.js`) |
| `go-stun` (Go, 2018) | MIT | Formato do Binding Response com `XOR-MAPPED-ADDRESS`, para o `logi-encontro`. STUN e QUIC no mesmo socket se separam pelo primeiro byte (RFC 7983) |
| `Parsec-Cloud-Preparation-Tool` (PowerShell) | sem licença: só referência | Lista de ajustes de VM: auto-logon, tirar o bloqueio, desligar o adaptador básico de vídeo, instalar o driver de monitor virtual |
| `Cloud-GPU-Updater` (PowerShell) | sem licença: só referência | GPU de nuvem em modo TCC não tem saída de vídeo; trocar para WDDM é um item do `doctor` para host em VM |

Nenhum deles cobre o lado do host que nos falta: captura de áudio, tela de bloqueio e
UAC, Ctrl+Alt+Del e o serviço SYSTEM. O "MouseKeys como cursor" do Preparation Tool
foi medido aqui e não funciona (`.claude/rules/cursor-without-mouse.md`).

## O que esta spec não decide

- O preço em reais, e o que entra no limite do relay grátis.
- Conta de usuário e lista de máquinas na nuvem: começa sem conta, só com
  id e pareamento.
- O endpoint que recebe a telemetria (vem com o `logi-encontro`).

## Veja também

- [[008-security]] — o trust model que esta spec muda
- [[012-outside-the-lan]] — por que overlay, e o que falta
- [[016-session-and-visuals]] — a sessão em aba, que passa a levar a digital
