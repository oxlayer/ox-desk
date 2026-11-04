# O DXGI não captura a tela de bloqueio, e isso não é bug nosso

`DuplicateOutput` devolve `E_ACCESSDENIED (0x80070005)` quando o
**desktop seguro** está na frente — tela de bloqueio ou diálogo de UAC.
É por desenho do Windows: aquele desktop é outro objeto, e um processo
do desktop `Default` não o enxerga.

## Diagnóstico: três causas, mesmo código de erro

Antes de culpar a tela bloqueada, descarte as outras duas. O erro é
idêntico nos três casos.

```powershell
# 1. tela de bloqueio / UAC na frente?
Get-Process -Name LogonUI -ErrorAction SilentlyContinue   # presente = bloqueada
Get-Process -Name consent -ErrorAction SilentlyContinue   # presente = UAC

# 2. pilha de vídeo meio instalada?
Get-CimInstance Win32_VideoController |
  ForEach-Object { $_.Name + " [" + $_.Status + "] cfg=" + $_.ConfigManagerErrorCode }
# um "Microsoft Basic Display Adapter" com status != OK = driver pela metade

# 3. reinício pendente?
Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending'
```

Em 04/11 o caso 2 era real (o driver virtual do Parsec saiu e deixou um
adaptador em estado `Unknown`) e depois se resolveu sozinho. Quando o
usuário reportou de novo, o adaptador estava **OK** e a causa era a
tela de bloqueio. Mesmo erro, causa diferente — por isso o roteiro.

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
- [[verificar-o-que-chegou]] — por que o log do host é a fonte, não o cliente
