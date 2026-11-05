# 013 — Binários por CI

> Estado: implementado. Linux e Windows conferidos contra o que este
> Ubuntu compila; o pacote do Mac foi conferido por formato e carimbo,
> mas **ainda não rodou no MacBook** (estava fora do ar no dia).

## O problema

Os binários só existiam como efeito colateral do `deploy.sh`: compilados
na máquina de quem dispara, e o Mac compilando a partir de um `rsync`.
Se o Mac está fora do ar, ninguém descobre que o código dele parou de
compilar. E não havia como responder "qual binário corresponde a este
commit?" sem recompilar.

## A forma

Um workflow, [`binarios.yml`](../.github/workflows/binarios.yml), que
chama um script, [`construir-binarios.sh`](../scripts/construir-binarios.sh).

| Alvo | Runner | Como compila | Pacote |
|---|---|---|---|
| Ubuntu | `ubuntu-24.04` | nativo, e roda os testes | `logi-linux-x86_64-<n>-<hash>.tar.gz` |
| Windows | `ubuntu-24.04` | cruzado, `x86_64-pc-windows-gnu` | `logi-windows-x86_64-unsigned-<n>-<hash>.tar.gz` |
| macOS | `macos-15` (arm64) | nativo, ffmpeg do brew | `logi-mac-arm64-unsigned-<n>-<hash>.tar.gz` |

| Evento | O que acontece |
|---|---|
| push em `main` ou `ci/**` que toca código | compila os três, pacotes ficam 14 dias como artefato |
| tag `v*` | o mesmo, e publica uma Release com os três pacotes e `SHA256SUMS` |
| `workflow_dispatch` | o mesmo que push, sob demanda |

Commit só de documentação não dispara nada: o filtro de `paths` existe
para não gastar minuto com isso.

## O que a CI não faz

**Não assina.** A chave do Authenticode não sai desta máquina
([`008`](008-security.md)), e subir a chave para um segredo do GitHub
trocaria o modelo de confiança por conveniência. A identidade do Mac
vive num chaveiro de lá. Por isso os pacotes de Windows e Mac levam
`unsigned` no nome:

- `.exe` cru chegando ao Windows é o que o Defender apaga na escrita;
- binário ad-hoc no Mac perde a concessão de Gravação de Tela.

**Não instala.** O `deploy.sh` continua sendo o único caminho até as
máquinas: ele assina, troca, reinicia e confere o `/healthz`. Fazer a CI
entregar exigiria um runner dentro da LAN ou uma porta aberta para fora,
e a regra é tudo na LAN. A exceção da
[`012`](012-fora-da-lan.md) é estreita e não cobre isto.

A regra de nada de nuvem ([`008`](008-security.md)) vale para o sistema
em execução: nenhum dado das máquinas passa pelo GitHub. O que sai daqui
é o código, que já estava lá.

**Não empacota a casca Electron.** O `gui/` não tem lógica, e o deploy
leva só o `main.js`.

## O que é conferido

"Compilou" não basta ([[../.claude/rules/verificar-o-que-chegou]]). Antes
de empacotar, o script recusa se:

- falta algum binário da lista, ou tem menos de 100 KB;
- o formato não é o do alvo (ELF, PE32+, Mach-O arm64);
- o `logid` não carrega o carimbo do commit, a mesma pergunta que o
  deploy faz ao `/healthz`, só que antes de qualquer máquina receber;
- no Linux, `ldd` acusa biblioteca ausente;
- no Windows, o `.exe` importa DLL que o sistema não tem (mingw, ffmpeg);
- no Mac, o `logid` depende do brew.

O tamanho e o sha256 de cada binário saem no log e no resumo do run. O
carimbo errado foi testado de propósito: `deadbee` é recusado e nada é
empacotado.

## Decisões

**Windows cruzado, e não `windows-latest`.** É o mesmo caminho do
`deploy.sh`, então o binário da CI é do mesmo tipo do que já roda lá. O
minuto Linux também custa metade do Windows.

**Tar, e não binário solto.** O `upload-artifact` perde o bit de
execução. Conferido no pacote baixado: `-rwxr-xr-x`.

**Rust fixado em `1.99.1`**, a versão deste Ubuntu. Sobe de propósito,
junto com a máquina, e não sozinho numa terça-feira.

**Actions fixadas por SHA.** Tag de action é mutável, e o job de release
tem permissão de escrita. `contents: write` existe só nele.

**Runner `ubuntu-24.04` num Ubuntu 26.04.** Compilar no glibc mais velho
roda no mais novo; o contrário não.

**`construir-binarios.sh` roda local.** A lista de binários não pode
viver só num YAML que ninguém executa. `--sem-compilar` confere o que já
está em `target/`.

## Limites conhecidos

- **A lista de binários é duplicada** entre `deploy.sh` e
  `construir-binarios.sh`. Binário novo entra nos dois. É o defeito que
  já deixou o `logistream-kvm` fora de um deploy; juntar as duas listas
  fica para quando o deploy passar a consumir os pacotes.
- **O host e o view do Mac ligam no ffmpeg do brew.** O run usou o
  9.0.2. O pacote só roda com o mesmo major instalado no MacBook, o que
  não foi conferido.
- **Os testes rodam só no Linux**, e do `logistream` só `--lib`. No Mac
  custaria dez vezes mais por minuto.
- **O orçamento de conversão não barra a CI.** O
  `conversao_cabe_no_orcamento` passou no primeiro run e caiu no terceiro
  com 12,18 ms contra o teto de 12, sem mudança no código medido: runner
  compartilhado não é o hardware para o qual o teto foi escrito. Ele
  segue rodando e imprimindo o número, sem decidir. Como o `check.sh`
  também não testa o `logistream`, **hoje nada automático barra essa
  regressão**: quem mexer na conversão roda o teste aqui.
- **Teste em runner sem GPU não cobre captura nem NVENC.** O que depende
  de sessão gráfica continua sendo verificado nas máquinas.

## Custo

O repo é privado no plano Free da organização: 2.000 minutos por mês, e
o minuto de macOS conta **dez vezes**. Cada job é arredondado para cima.

Medido no primeiro run, sem cache (05/11):

| Job | Duração | Minutos cobrados |
|---|---|---|
| linux, com 116 testes | 5min09 | 6 |
| windows | 3min45 | 4 |
| mac | 2min24 | 3 × 10 = 30 |
| **total** | | **40** |

São cerca de 50 pushes de código por mês no pior caso. O Mac é três
quartos da conta. Se o orçamento apertar, o primeiro corte é tirá-lo do
push e deixá-lo em tag e `workflow_dispatch`.

## Veja também

- [[008-security]] — por que a chave não sobe
- [[../.claude/skills/deploy]] — o caminho até as máquinas
- [[../.claude/rules/verificar-o-que-chegou]]
