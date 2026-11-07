# `E_ACCESSDENIED` no DXGI: quatro causas, e a mais comum é o protetor de tela

`DuplicateOutput` devolve `E_ACCESSDENIED (0x80070005)` quando o
processo **não está no desktop de input corrente**. O Windows tem
vários desktops por sessão, e o DXGI só duplica para quem está no que
está recebendo input.

## O protetor de tela é a causa número um, e custou três diagnósticos

Em 04/11 o usuário não conseguia alcançar o Windows. Diagnostiquei
"tela bloqueada"; depois "driver de vídeo pela metade". As duas
erradas: `LogonUI` **ausente**, RTX 3050 com status OK e `cfg=0`, e
mesmo assim recusa.

Era o `Bubbles.scr` rodando. O protetor vive num desktop próprio
("Screen-saver"), que vira o desktop de input — e o nosso processo, no
`Default`, perde acesso.

O que me tirou do buraco foi **listar processos** em vez de testar mais
uma hipótese:

```powershell
Get-Process | Where-Object { $_.Path -like '*.scr' }
```

Duas armadilhas que gastaram tempo:

- `SendInput` **não** dispensa o protetor. A injeção vai para o desktop
  da thread que chama (`Default`); o protetor, noutro desktop, nem fica
  sabendo. Por isso `acordar_tela()` resolve tela apagada e não resolve
  esta — são dois problemas que pareciam um.
- `SPI_GETSCREENSAVERRUNNING` consultado pelo SSH devolve `False`
  mesmo com o protetor no ar, porque a sessão 0 não enxerga o desktop
  da sessão 1. A lista de processos enxerga.

Hoje o host encerra qualquer `*.scr` na partida
(`encerrar_protetor_de_tela()`), que é o que as ferramentas de acesso
remoto fazem. Protetor de tela não tem estado a perder.

## Diagnóstico: quatro causas, mesmo código de erro

Em ordem de frequência medida.

```powershell
# 1. protetor de tela rodando?  (a causa mais comum)
Get-Process | Where-Object { $_.Path -like '*.scr' }

# 2. tela de bloqueio / UAC na frente?
Get-Process -Name LogonUI -ErrorAction SilentlyContinue   # presente = bloqueada
Get-Process -Name consent -ErrorAction SilentlyContinue   # presente = UAC

# 3. pilha de vídeo meio instalada?
Get-CimInstance Win32_VideoController |
  ForEach-Object { $_.Name + " [" + $_.Status + "] cfg=" + $_.ConfigManagerErrorCode }
# um "Microsoft Basic Display Adapter" com status != OK = driver pela metade

# 4. reinício pendente?
Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending'
```

O driver pela metade foi real de manhã (o adaptador virtual do Parsec
saiu e deixou um `Microsoft Basic Display Adapter` em `Unknown`) e
depois se resolveu sozinho. À tarde o adaptador estava OK e a causa era
o protetor. **Mesmo erro, causa diferente, no mesmo dia** — por isso o
roteiro existe, e por isso ele começa pela lista de processos em vez de
pela hipótese favorita.

## Tela APAGADA é outra coisa, e essa nós resolvemos

Não confundir com economia de energia tendo desligado a saída de vídeo.
Ali o `DuplicateOutput` **passa**, e o sintoma é pior de achar: o DXGI
devolve `Parado` para sempre, nunca existe um primeiro quadro, e o
cliente fica no preto sem ninguém reclamar.

Resolvido em `host_win.rs`: `acordar_tela()` na partida (um
`MOUSEEVENTF_MOVE` de +1 e -1 na mesma chamada — um de (0,0) é no-op e
o Windows descarta), `TelaAcesa` segurando `ES_DISPLAY_REQUIRED`
enquanto servimos, e um vigia que cutuca aos 3s e recria a duplicação
aos 9s se nenhum quadro aparecer.

## O que seria preciso para capturar a tela de bloqueio

Serviço rodando como SYSTEM na sessão 0 que descobre a sessão de
console ativa (`WTSGetActiveConsoleSessionId`), pega o token
(`WTSQueryUserToken`) e lança o capturador com `CreateProcessAsUser`
apontando `lpDesktop` para o desktop corrente — `Winlogon` quando
bloqueada, `Default` quando não. E relança quando o desktop troca.

É o que Parsec e AnyDesk fazem. Não está construído.

## Veja também

- [[../../specs/010-remote-desktop.md]] — o stack de tela inteiro
- [[verify-what-arrived]] — por que o log do host é a fonte, não o cliente
