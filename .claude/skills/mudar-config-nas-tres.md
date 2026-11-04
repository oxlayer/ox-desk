# Mudar config nas três máquinas

Config não viaja no deploy. Mudar uma e achar que mudou todas é o erro
padrão — ver [[../rules/config-por-maquina]].

A regra do procedimento: **uma transformação, aplicada três vezes**.
Editar as três à mão introduz divergência por digitação.

## 1. Escreva a transformação como script

```python
# tirar-rdp.py — exemplo real de 04/11
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
s = p.read_text()
antes = s
s = s.replace('units = ["logid.service", "gnome-remote-desktop.service"]',
              'units = ["logid.service"]')
s = re.sub(r'\n *\[\[hosts\.services\]\]\n *label = "RDP[^"]*"\n *port = 3389\n(?: *expect = [^\n]*\n)?', '\n', s)
p.write_text(s)
print("mudou" if s != antes else "ja estava limpo", "->", p)
```

Imprimir "mudou" vs "já estava limpo" é o que torna a operação
idempotente e auditável: rodar duas vezes não estraga, e o silêncio
numa máquina denuncia que o padrão não bateu lá.

## 2. Ubuntu (local)

```bash
python3 -I tirar-rdp.py ~/.config/logi/config.toml
systemctl --user restart logid.service
```

## 3. macOS

```bash
scp mac:.config/logi/config.toml /tmp/mac-config.toml
python3 -I tirar-rdp.py /tmp/mac-config.toml
scp /tmp/mac-config.toml mac:.config/logi/config.toml
ssh mac 'launchctl kickstart -k gui/$(id -u)/ai.logi.logid'
```

## 4. Windows

O caminho é `%APPDATA%\logi\`, que em `scp` vira
`C:/Users/user/AppData/Roaming/logi/`.

```bash
scp 'windows-host:C:/Users/user/AppData/Roaming/logi/config.toml' /tmp/win-config.toml
python3 -I tirar-rdp.py /tmp/win-config.toml
scp /tmp/win-config.toml 'windows-host:C:/Users/user/AppData/Roaming/logi/config.toml'
```

Reiniciar o agente — o processo é **`logid.exe`** e a tarefa é
**`logid`**, e `taskkill` em vez de `Stop-Process`
([[../rules/reiniciar-agente]]):

```powershell
taskkill /F /IM logid.exe
Start-Sleep -Seconds 2
schtasks /run /tn logid
```

## 5. Conferir as três

Não confie no "ok" de cada passo:

```bash
curl -s localhost:8787/api/status                     # ubuntu
ssh mac 'curl -s localhost:8787/api/status'           # mac
# windows: Invoke-WebRequest http://127.0.0.1:8787/api/status
```

Se uma ainda mostra o estado velho, o agente dela não reiniciou.

## E o `default-config.toml`

Atualize também `logid/default-config.toml` no repo — é a semente de
instalação nova. Não muda nenhuma máquina que já roda, mas sem isso a
próxima instalação ressuscita o que você acabou de remover.
