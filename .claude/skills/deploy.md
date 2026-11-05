# Deploy nas três máquinas

```bash
cd ~/dev/logi && ./scripts/deploy.sh
```

Compila para Ubuntu, faz cross-compile e assina para Windows, manda
fonte e compila no Mac, reinicia tudo, e termina conferindo que as três
reportam o mesmo build.

## Ler a saída

O rodapé é o que importa:

```
== conferencia ==
  mac        #75 d72074b
  windows    #75 d72074b
  ubuntu     #75 d72074b

tres maquinas no mesmo build: d72074b
```

Qualquer `FORA DE SINCRONIA` significa que aquela máquina ficou para
trás — normalmente porque um passo falhou mais acima e o `ok` dele era
mentira. Todo passo do Windows captura stderr e confere o tamanho do
arquivo que aterrissou, justamente porque isso já aconteceu
([[../rules/verificar-o-que-chegou]]).

## O que o deploy NÃO leva

- **Config.** Ver [[mudar-config-nas-tres]].
- **Regras de udev.** Ver [[dar-acesso-a-input]].

## Binários da CI não substituem o deploy

A CI compila os três sistemas a cada push de código, mas **não assina
nem instala**. Pacote `unsigned` não vai direto para o Windows nem para
o Mac. Ver [`specs/013`](../../specs/013-ci-cd.md).

## Commits de docs não precisam de deploy

O build id é o HEAD do git, então um commit só de documentação faz as
máquinas reportarem "atrasadas" sem nada de funcional ter mudado. Não é
problema; o próximo deploy sincroniza.

## Reiniciar a casca Electron (bandeja)

O menu da bandeja vive no **processo principal** do Electron. O
`vigiaBuild()` recarrega a *página* quando o build muda, mas não o
processo principal — então mudança em `gui/main.js` só entra
reiniciando o app.

```bash
pkill -f 'logi/gui/node_modules/electron/dist/electron'
cd ~/dev/logi/gui && (setsid nohup env -u ELECTRON_RUN_AS_NODE \
  node ./node_modules/.bin/electron . > ~/.local/state/logi-gui.log 2>&1 &)
```

**O padrão era `electron \.$` e isso falhava calado.** Com qualquer
argumento (`electron . --setup`) a linha de comando não termina em `.`
— e o próprio binário reordena os argumentos para `electron --setup .`.
O `pkill` não casava, a instância velha continuava viva, a nova batia
no `requestSingleInstanceLock()` e saía, e o log vinha vazio porque o
`>` acabara de truncá-lo. Sintoma: "não subiu", com o app rodando o
tempo todo. O caminho do binário casa sempre.

**`env -u ELECTRON_RUN_AS_NODE` não é opcional.** Com essa variável
no ambiente, o electron roda como node puro, `app` vem `undefined` e
morre em `app.requestSingleInstanceLock()` — mensagem que não diz nada
sobre a variável. Já derrubei a GUI do usuário assim.

Conferir que voltou, sempre — matar e não relançar deixa a pessoa sem
bandeja:

```bash
# `pgrep -af electron` casa com o PRÓPRIO comando que você digitou
# (a string "electron" está nele) e sempre "acha" algo. Filtrar por
# `comm` olha o executável, não a linha de comando.
ps -eo pid,comm --no-headers | awk '$2 ~ /electron/' | head -3
tail -3 ~/.local/state/logi-gui.log
```

O aviso de Vulkan/ozone no log é cosmético.

## Abrir direto na configuração

```bash
node ./node_modules/.bin/electron . --setup
```

Abre em tela cheia na página de setup. É o mesmo que a bandeja faz em
"Configurar esta máquina" — ver [[configurar-maquina]].
