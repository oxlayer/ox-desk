# Cada máquina tem a própria config, e nada sincroniza

Três máquinas, três `config.toml`, **zero sincronização**. O
`deploy.sh` leva binários; config não. Mudar uma e achar que mudou
todas é o erro padrão.

| Máquina | Caminho |
|---|---|
| Ubuntu | `~/.config/logi/config.toml` |
| macOS | `~/.config/logi/config.toml` (via `ssh mac`) |
| Windows | `%APPDATA%\logi\config.toml` (`C:/Users/user/AppData/Roaming/logi/`) |

`logid/default-config.toml` é só semente de instalação nova. Editar lá
**não** muda máquina nenhuma que já roda.

## Como isso apareceu

Tiramos o RDP do painel editando a config do Ubuntu e o
`default-config.toml`. O painel do Ubuntu limpou. O usuário respondeu:
*"kdp sumiu do ubuntu mas ainda vejo na gui do windows e mac"* — porque
cada agente lê a config da própria máquina e serve o próprio painel.

## Procedimento

Ver [[../skills/change-config-on-all-three]]. Em resumo: baixar, transformar
com o MESMO script nas três, subir, reiniciar o agente de cada uma.
Nunca editar as três à mão — a divergência entra por digitação.

## Veja também

- [[restart-the-agent]] — os nomes de processo e tarefa diferem por SO
- [[../skills/change-config-on-all-three]] — a receita
