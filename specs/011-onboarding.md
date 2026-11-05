# 011 — Configurar uma máquina

> Estado: implementado (Ubuntu e Windows medidos; macOS escrito e **não
> verificado** — o MacBook estava fora do ar no dia).

## O problema

Montar uma máquina deste conjunto é uma lista de oito a onze coisas,
diferente em cada sistema. O que torna a lista cara não é o tamanho: é
que **nenhum dos itens dá erro claro quando falta**.

| O que falta | O que a pessoa vê |
|---|---|
| ACL no `/dev/hidraw` | o painel não acha dispositivo nenhum |
| Gravação de Tela (macOS) | o log para em `abrindo a tela 1` e a última linha fala de formato de pixel |
| Acessibilidade (macOS) | a tela aparece e o ponteiro não anda |
| Tarefa sem `/IT` (Windows) | `E_ACCESSDENIED` ou captura preta |
| Chave em `~/.ssh` em vez de `administrators_authorized_keys` | `Permission denied`, sem pista |
| Binário ad-hoc no Mac | tudo funciona até o próximo deploy, e aí para |

Todos os sintomas acima chegam como "não funcionou". O `status.sh` já
respondia parte disso, mas só no Linux, só no terminal, e só para quem
sabe que ele existe.

## A forma

Uma página servida pelo próprio daemon em **`/setup`**, que mede o que
falta nesta máquina e arruma o que dá para arrumar. A casca Electron a
abre em tela cheia na partida **quando falta algo essencial**, e a
bandeja tem o item "Configurar esta máquina".

Roda **de dentro do daemon**, e isso é metade do desenho: o daemon vive
na sessão gráfica (LaunchAgent no Mac, tarefa `/IT` no Windows, unit de
usuário no Linux), então ele enxerga o que uma sessão SSH não enxerga.
Medir por SSH daria "não" para coisas que estão certas — ver
[[../.claude/rules/sessao-grafica]].

## As três regras

### 1. Não medir é um estado, e ele bloqueia

`Estado::Erro` existe ao lado de `Ok`, `Falta` e `Fora`, e pesa igual a
`Falta` no resumo. Um passo que o wizard não conseguiu medir **não é um
passo aprovado**. Um passo que ninguém mediu também não: `estado_de`
devolve `Erro` para id ausente, e há teste para isso.

Essa é a mesma família de defeito do `backfill_gate` que não gateava e
do `catch` que engolia o `MODULE_NOT_FOUND`: o no-op silencioso parece
sucesso.

### 2. Arrumar não é ter arrumado

Todo reparo **mede de novo** e devolve o que a medição disse, não o que
o comando devolveu. O `assinar.sh` do Mac é deliberadamente não-fatal; o
`schtasks` devolve 0 criando tarefa sem token interativo; o `electron`
sobe e morre em 200 ms. Nos três casos o código de saída mente e a
medição não ([[../.claude/rules/verificar-o-que-chegou]]).

### 3. Quem precisa de humano diz que precisa

Quatro tipos de reparo, e a diferença é quem age:

| Tipo | Quem faz | Exemplo |
|---|---|---|
| `sozinho` | o daemon, agora | criar a tarefa agendada, habilitar a unit |
| `sudo` | a pessoa, em qualquer terminal | instalar a regra de udev |
| `clique` | a pessoa, **na frente da máquina** | TCC do macOS |
| `receita` | a pessoa, com a receita escrita | rodar o deploy |

Misturar `sudo` com `clique` seria prometer automação que não existe: o
primeiro resolve por SSH, o segundo exige consentimento humano numa
janela do sistema.

## Onde fica o quê

| Arquivo | Papel |
|---|---|
| `logid/src/setup.rs` | catálogo dos três sistemas + decisão. **Puro**, com testes |
| `logid/src/setup_comum.rs` | medições iguais nas três (config, pares, clipboard) |
| `logid/src/setup_linux.rs` | binários, unit, udev, Wayland, NVENC, bandeja |
| `logid/src/setup_mac.rs` | launchd, identidade, assinatura, TCC, Login Remoto |
| `logid/src/setup_win.rs` | tarefas `/IT`, sshd, chave de admin, protetor de tela |
| `logid/static/setup.html` | a página |
| `gui/check-setup.js` | teste de fumaça da página contra a API |

O catálogo é puro de propósito: os testes rodam no laptop de
desenvolvimento cobrindo os **três** sistemas, não só o que está
compilando. É o que pega dependência pendurada e passo fora de ordem
antes de existir uma máquina para testar.

## O que o catálogo garante (testado)

- ids únicos em cada sistema
- toda dependência **existe** e **vem antes** no catálogo — senão o
  passo espera para sempre, ou a tela mostra "esperando" acima da coisa
  esperada
- todo passo diz por que existe (`porque` com mais de 40 caracteres).
  Um passo sem motivo vira clique automático
- `Fora` (não se aplica) não conta nem bloqueia; `Erro` bloqueia

## A dependência que mais importa

No macOS, `gravacao-de-tela` e `acessibilidade` **dependem de**
`assinatura`. Não é zelo de ordem: com binário ad-hoc a amarração do TCC
é pelo cdhash, que muda a cada compilação. Conceder antes de assinar
desperdiça a concessão, e só o dono da máquina pode dar de volta, à mão,
na interface. Isso já aconteceu duas vezes aqui — ver
[[../.claude/rules/espaco-de-coordenadas]] e o `mac/criar-identidade.sh`.

## O que o wizard NÃO faz

- **Não instala binário.** Compilar leva minutos e exige cargo; o
  caminho é `install.sh` ou `scripts/deploy.sh`. O wizard diz qual
  rodar.
- **Não roda sudo.** O daemon é serviço de usuário. Pedir senha de root
  numa página web local seria ensinar um hábito ruim por economia de
  dois comandos.
- **Não mede sozinho em laço.** O passo `pares` abre SSH para cada
  máquina; um temporizador de 10 s transformaria uma pergunta mensal
  numa tempestade de conexões. A página mede ao abrir, depois de cada
  reparo, e quando a pessoa pede.

## Pendente

- A parte do macOS está escrita e **não foi executada**: o MacBook não
  resolvia por mDNS no dia. O que falta conferir lá é `--conferir`
  compilando, o `launchctl print` como medida, e se o diálogo do TCC
  aparece quando disparado pelo daemon (que roda na sessão gráfica,
  então deveria).
- `logid/udev/70-logid-hidraw.rules` entrou junto e nunca foi
  instalado em máquina nenhuma: aqui quem concede é a regra do pacote
  do Solaar, que continua em `/usr/lib` mesmo com o Solaar removido.

## Veja também

- [[001-control-plane]] — o daemon que serve esta página
- [[008-security]] — por que a API só escuta em 127.0.0.1
- [[../.claude/skills/configurar-maquina]] — a receita de uso
