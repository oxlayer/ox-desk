# Reiniciar agente: o nome do processo não é o nome do binário

Cada SO tem seu mecanismo, e os nomes **não** seguem padrão. Errar o
nome falha em silêncio: `Stop-Process` de processo inexistente não é
erro, e `schtasks /run` numa tarefa já rodando não faz nada. O
resultado é "reiniciei" sem ter reiniciado.

| Máquina | Processo | Comando |
|---|---|---|
| Ubuntu | `logid` | `systemctl --user restart logid.service` |
| macOS | `logid` | `launchctl kickstart -k gui/$(id -u)/ai.logi.logid` |
| Windows | **`logid.exe`** | `taskkill /F /IM logid.exe` + `schtasks /run /tn logid` |

A tarefa agendada no Windows chama-se **`logid`**, e o executável
**`logid.exe`** — não `logid-agent`, apesar de o deploy falar em
`logid-agent.exe` em outros contextos. Conferir com
`schtasks /query /fo table | findstr /i logi`.

## `taskkill`, não `Stop-Process`

Pelo SSH, `Stop-Process -Force` **não mata processo da sessão
interativa**. Um `logistream-host` sobreviveu a três tentativas e
segurou a porta 9777, fazendo os testes seguintes falharem por motivo
errado — perdi tempo achando que a captura estava quebrada.

`taskkill /F /IM nome.exe` mata.

## Verificar, sempre

Reiniciar sem conferir é meio trabalho. Depois do restart, bater na API:

```
curl -s localhost:8787/api/status
```

Se o painel ainda mostra o estado velho, o agente não reiniciou.

## Veja também

- [[config-por-maquina]] — por que você está reiniciando
- [[powershell-remoto]] — como mandar isso sem quebrar nas aspas
