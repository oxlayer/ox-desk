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

## Commits de docs não precisam de deploy

O build id é o HEAD do git, então um commit só de documentação faz as
máquinas reportarem "atrasadas" sem nada de funcional ter mudado. Não é
problema; o próximo deploy sincroniza.
