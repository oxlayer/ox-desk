# 009 — Gerenciador de tarefas

Pedido do usuário: *"ver estatísticas, uso e conseguir finalizar tarefa
também do outro pc"*.

`GET /api/processes?host=<nome>` e `POST /api/processes/kill`, mais o
painel **Tarefas** no dashboard. Funciona para os três hosts, local ou
por SSH.

## Um comando por SO, escolhido pelo SO do *alvo*

Não existe comando que os três entendam. A escolha sai de
`HostCfg::os` — não do SO onde o daemon roda. Rodar `ps` contra o
Windows, ou `Get-Process` contra o Ubuntu, falha **calado**: o painel
ficaria vazio sem dizer por quê.

| alvo | comando |
|---|---|
| Linux / macOS | `ps -eo pid=,pcpu=,rss=,comm= \| sort -k2 -rn \| head -n 60` |
| Windows | `Get-Process \| Sort CPU -Desc \| Select -First 60 … \| ConvertTo-Json` |

A ordenação sai no `sort` e não na flag do `ps` porque elas divergem:
Linux tem `--sort=-pcpu`, macOS tem `-r`.

## `cpu` significa coisas diferentes, e isso é declarado

No Unix o `ps` dá **percentual instantâneo**. No Windows o `Get-Process`
dá **segundos acumulados** de CPU desde que o processo subiu. São
unidades diferentes e leituras diferentes.

Converter exigiria duas amostras e não vale o custo. Então o JSON traz
`cpu_kind` (`pct` ou `seconds`) e a UI troca o cabeçalho da coluna entre
`cpu %` e `cpu s`. Mostrar `24715` numa coluna `%` seria mentira
plausível — a pior espécie, porque ninguém desconfia.

## Encerrar pede confirmação, em dois passos

Mesma postura do `/api/focus` e do `set-host`: matar processo na máquina
de alguém não sai de um clique distraído, e não tem desfazer. O corpo
exige `confirm: true`, e a UI arma antes de agir — o primeiro clique
troca o botão por `encerrar | forçado`, o segundo age.

`pid` 0 e 1 são recusados no servidor.

No Windows o `taskkill` leva `/T` (a árvore): fechar só o pai deixaria
filho órfão rodando, e quem clicou esperava que aquilo sumisse.

## `remote.rs`: a terceira vez virou função

O `probe::ssh_detail` e o `agent` já montavam `ssh` com as mesmas quatro
opções mais o contorno de resolução ([`002`](002-ssh.md)). Este módulo
seria a terceira cópia — e a quarta seria onde alguém esqueceria o
`ssh_addr_opts` e quebraria só no Windows, que é exatamente como aquele
bug apareceu da primeira vez.

`remote::run(host, cmd, timeout)` resolve local contra remoto, escolhe
`sh -c` ou `powershell -Command`, e filtra o aviso de post-quantum que o
OpenSSH do Windows cospe em stderr.

## Verificado

Nos três, pelo painel:

- `ubuntu-host` — `local`, `cpu %`, 60 processos
- `windows-host` — `ssh`, `cpu s`, 60 processos
- `mac-host` — `ssh`, `cpu %`, 60 processos

E a confirmação em dois passos acompanha o pid certo mesmo quando a
lista reordena entre um clique e outro.

## Falta

- Totais da máquina (CPU agregada, memória) no cabeçalho do painel. O
  `/api/status` já traz RAM livre/total; falta costurar.
- Filtro por nome.
