# Um supervisor por daemon — e nunca a GUI

Custou um dia inteiro. O Mac serviu um binário de dez dias atrás
enquanto **todo** passo do deploy dizia `ok`, e quem notou foi o
usuário.

## O que acontecia

A GUI (`gui/main.js`) subia o daemon por conta própria quando o
`/healthz` não respondia. Isso parece educado — "se ninguém atende, eu
subo" — mas cria um **segundo dono da porta 8787**:

```
boot → ai.logi.gui sobe primeiro → pinga, ninguém atende → spawn(logid)
     → ai.logi.logid (LaunchAgent) sobe depois → Address already in use
     → exit 1 → launchd throttla → state = spawn scheduled, para sempre
```

O vencedor da corrida é um **órfão**: `launchctl print` não o conhece,
`launchctl kickstart -k` não o mata, `systemctl restart` não o mata.
O perdedor é justamente a cópia que o deploy e o OTA sabem reiniciar.

Resultado: `install -m755` escrevia o arquivo novo, `kickstart -k`
reiniciava um job que morria em loop, o órfão seguia respondendo o
build velho — e cada passo devolvia exit 0.

## As três regras

### 1. Quem supervisiona é o SO

`systemd --user`, `launchd`, `schtasks`. A GUI **pede** (`systemctl
--user start`, `launchctl kickstart`, `schtasks /run`) e só sobe um
filho avulso se não houver supervisor instalado — caso de máquina nova,
onde é pela GUI que se chega ao assistente.

### 2. Quem reinicia limpa a porta primeiro

`pkill -x logid` antes do `kickstart -k` / `systemctl restart`. Não é
brutalidade: o `-k` já mata a cópia supervisionada de qualquer forma,
então o efeito líquido é exatamente *"também as que o supervisor não
conhece"*. Está em `plano::Plano::limpar` (OTA) e nos passos do
`deploy.sh`.

O OTA sem isso é pior que o deploy: ele reinicia, pergunta o
`/healthz`, ouve o build velho do órfão e **reverte uma atualização que
estava correta**.

### 3. O painel compara quem serve com o que está no disco

`logid versao` imprime o carimbo do **arquivo**; `env!("LOGI_BUILD")` é
o carimbo de **quem responde**. Divergência vira linha do painel
(`setup_comum::veredito_do_carimbo`). Existir no disco não é estar no ar.

E "job carregado" não é "job rodando": `launchctl print` traz `state` e
`last exit code`, e ignorá-los aprovava um crash-loop
(`setup_comum::veredito_do_launchd`). O painel dizia "carregado na
sessão gráfica" sobre um daemon que não subia desde o boot.

## Veja também

- [[verificar-o-que-chegou]] — mandar reiniciar não é ter reiniciado
- [[reiniciar-agente]] — a mesma lição no lado do Windows
- [[sessao-grafica]] — por que `launchctl` precisa do `gui/<uid>/`
