# Clipboard entre as máquinas

Texto sincronizado nas três, sem KDE Connect. Detalhes de desenho em
[[../../specs/004-clipboard.md]].

## Como conferir que está vivo

```bash
# Ubuntu -> pares
echo -n "teste-$(date +%s)" | xclip -selection clipboard
sleep 5
ssh mac 'pbpaste'
```

No Windows **não** dá para conferir por SSH: o clipboard é por window
station, e `Get-Clipboard` pela sessão 0 lê outro. Precisa de tarefa
interativa:

```powershell
schtasks /create /tn ler-clip /tr "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$ps`"" /sc once /st 00:00 /it /f
schtasks /run /tn ler-clip
```

onde o script faz `Get-Clipboard -Raw | Out-File ...` e você lê o
arquivo. Mesma fronteira de sessão que aparece no cursor e no host de
tela ([[../rules/cursor-without-mouse]]).

## Quando não propaga

O log do `logid` de origem diz. `journalctl --user -u logid -n 30 | grep clip`:

| Mensagem | Causa |
|---|---|
| `command not found: logid` | caminho remoto errado — o shell do SSH não carrega o PATH do login |
| `recusou (exit status: 1)` com stderr vazio | o `logid` do par é velho e não conhece `clip-set` — faça deploy |
| `nao consegui falar com <host>` | SSH não conecta: chave, mDNS ou máquina fora |
| nada no log | a origem não viu a cópia; no Windows/Mac a amostragem é de 2 s |

## A chave do Mac

O Mac alcança o Ubuntu pelo alias `ubuntu` do `~/.ssh/config` dele, que
carrega a `IdentityFile` certa. Testar com `user@ubuntu-host.local`
dá `Permission denied` mesmo com tudo certo — o alias é que tem a
chave. Use sempre o alias que a config do `logid` declara em `ssh = `.

Se a chave não estiver autorizada deste lado:

```bash
ssh mac 'cat ~/.ssh/id_ed25519_logi.pub' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

## Limites

Só **texto**, teto de 256 KB. Imagem e arquivo não viajam — o KDE
Connect fazia isso; se fizer falta, é trabalho novo.

---

# Clipboard de arquivos

Mesmo caminho do texto (SSH, sem porta nova), mas a natureza é outra:
texto **é** o conteúdo; arquivo é uma referência, e o clipboard do
destino tem de apontar para arquivos que existem no disco dele. Por
isso os bytes viajam no **Ctrl+C**, não no Ctrl+V — não há como
interceptar a colagem nos três sistemas.

## Os três limites

| Limite | Valor | O que decide |
|---|---|---|
| `TETO_EMBUTIDO` | 8 MiB | vai sozinho para todos os pares |
| `TETO_TOTAL` | 2 GiB | acima disto é acidente, recusa |
| `TETO_ARQUIVOS` | 2000 | idem, para árvore grande e leve |

Entre 8 MiB e 2 GiB **não** mandamos sozinhos: a bandeja mostra
"Mandar arquivos para `<host>`" e você escolhe um destino. Copiar 1,4 GB
para duas máquinas que ninguém está olhando gasta banda e disco alheio.

Acima de 8 MiB para um destino unix vai por `rsync --partial`; com
Windows em qualquer ponta vai por `tar`, porque o Windows tem
`tar.exe` e `scp.exe` em `system32` mas **não tem rsync** (medido).

## Testar sem GUI

Copiar arquivo no Ubuntu sem clicar no Nautilus:

```bash
printf 'file://%s\r\n' "$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1]))" "/caminho/com espaço.txt")" \
  | xclip -selection clipboard -t text/uri-list
```

No Windows e no macOS a cópia tem de nascer na **sessão gráfica**:

```bash
# Windows: tarefa /IT (pelo SSH é a sessão 0, clipboard próprio e vazio)
scp windows/sondas/por-arquivo-no-clipboard.ps1 'windows-host:AppData/Local/logid/'
ssh windows-host 'schtasks /create /tn por-arquivo /tr "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File $env:LOCALAPPDATA\logid\por-arquivo-no-clipboard.ps1" /sc once /st 00:00 /it /f; schtasks /run /tn por-arquivo'

# macOS: o pasteboard é por usuário, então pelo SSH funciona
ssh mac 'osascript -l JavaScript -e "ObjC.import(\"AppKit\");const pb=\$.NSPasteboard.generalPasteboard;pb.clearContents;pb.writeObjects(\$([\$.NSURL.fileURLWithPath(\"/tmp/x.txt\")]))"'
```

## Conferir o que chegou

```bash
# Ubuntu: os três tipos têm de aparecer
xclip -selection clipboard -t TARGETS -o
xclip -selection clipboard -t x-special/gnome-copied-files -o   # começa com `copy`

# Windows: sessão interativa, via tarefa /IT
ssh windows-host 'schtasks /run /tn ler-clipboard; Start-Sleep 3; Get-Content $env:LOCALAPPDATA\logid\clipboard.txt'

# macOS
ssh mac 'osascript -l JavaScript /tmp/ler-pb.js'
```

`x-special/gnome-copied-files` sempre começa com **`copy`**, nunca
`cut`: `cut` manda o destino apagar a origem depois de colar, e a
origem está em outra máquina. Não temos como saber que a colagem
aconteceu, e apagar arquivo de alguém por engano não tem desfazer.

## Armadilhas que já custaram tempo

- **PowerShell não espera processo GUI.** O clipboard de texto para o
  Windows estava intermitente desde sempre por isso. Ver
  [[../rules/remote-powershell]], armadilha #4.
- **Nome repetido é recusado**, não renomeado: dois `igual.txt` de
  pastas diferentes viriam um sobre o outro na pasta de passagem e você
  colaria um achando que são dois.
- **A pasta de passagem fica em `~/.cache/logi/clipboard/`**, fora de
  `/tmp`: em algumas distros `/tmp` é tmpfs, e 2 GB de clipboard na RAM
  é um jeito de achar o OOM killer. Guarda as 5 últimas entregas.
