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
tela ([[../rules/cursor-sem-mouse]]).

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
