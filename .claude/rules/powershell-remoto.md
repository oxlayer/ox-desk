# PowerShell remoto: sempre `-EncodedCommand` a partir de arquivo

Inline quebra. Não "às vezes": quebra toda vez que o comando tem aspas,
`$`, `` ` `` ou acento. Quebrou tantas vezes neste repo que virou regra.

```bash
# ✅ fazer
cat > "$S/tarefa.ps1" <<'PS1'
$p = Get-Process -Name logid -ErrorAction SilentlyContinue
if ($p) { "pid " + $p.Id } else { "nao esta rodando" }
PS1
ENC=$(iconv -f UTF-8 -t UTF-16LE < "$S/tarefa.ps1" | base64 -w0)
ssh windows-host "powershell -NoProfile -EncodedCommand $ENC"
```

```bash
# ❌ não fazer
ssh windows-host 'powershell -NoProfile -Command "if ($p) { \"x\" }"'
```

O heredoc tem de ser **quotado** (`<<'PS1'`): sem as aspas o bash
expande `$p` e executa `` ` `` como substituição de comando. Um
`` `r`n `` dentro de heredoc expandido já rodou como comando, e o passo
ainda disse "ok" porque o erro foi para stderr.

## Três armadilhas específicas

1. **`$args` é variável automática do PowerShell** e não pode ser nome
   de parâmetro.
2. **Saída vem com `\r`** — passar por `tr -d '\r'` antes de comparar.
3. **O cabeçalho CLIXML e os avisos de SSH poluem o stdout.** Filtrar:
   `grep -v "WARNING\|store now\|may need\|openssh\|CLIXML\|^<Objs"`.

## Veja também

- [[verificar-o-que-chegou]] — mandar não é ter chegado
