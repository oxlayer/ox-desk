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

## Armadilha #4: o PowerShell não espera executável GUI-subsystem

A mais caríssima das quatro, porque ela **não falha — ela sorteia**.

O `logid.exe` é `windows_subsystem = "windows"`, sem console, para o
daemon não piscar janela ao subir pela tarefa agendada. Mas o mesmo
binário serve de CLI, e o PowerShell **não espera** processo GUI
terminar. Dois estragos encadeados:

1. `$LASTEXITCODE` nunca é setado, então `; exit $LASTEXITCODE`
   devolve 0 e o SSH relata **sucesso** qualquer que seja o resultado.
2. A sessão SSH fecha enquanto o processo ainda está lendo o stdin.
   Quem ganha a corrida depende do tamanho do payload.

Medido em 04/11/2026, três envios de texto para `logid clip-set`:

| Comando remoto | Chegou |
|---|---|
| `& "...\logid.exe" clip-set` | **2 de 3** |
| `& "...\logid.exe" clip-set \| Out-String \| Write-Output` | 3 de 3 |

Consumir a saída obriga o PowerShell a esperar o fim do processo. Sem
isso, o clipboard de texto para o Windows estava **intermitente desde
sempre** e ninguém notou: uma linha de texto costuma ganhar a corrida,
e foi um `tar` de 50 KB — que perde sempre — que revelou o problema.

Use [`remote::comando_logid`](../../logid/src/remote.rs), que já monta
a linha certa por sistema. Não monte na mão.

```rust
// ✅ fazer
.arg(crate::remote::comando_logid(&host, "clip-arq-set"))

// ❌ não fazer
.arg(r#"& "$env:LOCALAPPDATA\logid\logid.exe" clip-arq-set"#)
```

### Corolário: stdin binário para um filho não sobrevive

No mesmo binário sem console, passar o stdin adiante com
`Stdio::inherit()` entrega **pasta vazia e exit 0**. Ler o stdin
*dentro* do processo funciona — é o que `clip-set` sempre fez, e por
isso é o caminho usado também em `clip-arq-set`, que grava um `.tar`
temporário em vez de deixar o `tar` herdar o descritor.

Verificação na sessão interativa: `windows/sondas/ler-clipboard.ps1`
via tarefa `/IT` (pelo SSH se lê o clipboard da sessão 0, sempre
vazio).

## Veja também

- [[verify-what-arrived]] — mandar não é ter chegado
