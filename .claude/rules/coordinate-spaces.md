# Pixel de captura ≠ ponto de injeção

O protocolo carrega `Evento::Mouse` em **pixel do quadro**: o
visualizador sabe o tamanho do que recebe e mapeia a janela para ele
(`ponto_remoto`, em `logistream/src/bin/view.rs`). O host, do outro
lado, injeta no espaço de coordenadas **do sistema** — e os dois não
são o mesmo espaço em toda máquina.

| Sistema | Captura | Injeção | Fator |
|---|---|---|---|
| macOS Retina | pixels do modo (3360×2100) | pontos do CGEvent (1680×1050) | **2** |
| Windows | pixels físicos (DXGI) | `GetSystemMetrics`, lógico se o processo não declara DPI | escala do display |
| Linux | pixels | pixels (Mutter) | 1 |

## macOS: estava quebrado, está corrigido

`CGEventCreateMouseEvent` trabalha em pontos. Mandar o pixel direto
leva o ponteiro ao dobro da posição: apontar no meio da janela joga o
cursor no canto inferior direito, porque o dobro de tudo é maior que a
tela e o sistema prende na borda. Chegou como *"a posição do cursor no
mac não bateu"*.

A conversão é `ponto_de_pixel` em `logistream/src/input.rs` (neutra de
plataforma, com testes), e a escala sai do **modo de vídeo**:

```rust
CGDisplayModeGetPixelWidth(modo) / CGDisplayModeGetWidth(modo)   // 2.0
```

**`CGDisplayPixelsWide` não serve para isso.** Apesar do nome, ela
devolve **pontos** — medido: 1680 num display cujo modo declara 3360
pixels. Foi o nome que deixou o bug passar: o clamp de borda do
movimento relativo usa essa função e por isso está certo *por
acidente*, e só o caminho absoluto quebrou.

O host diz a escala no log na partida, porque esse sintoma não se
diagnostica sem ela:

```
  2 pixel(s) de captura por ponto de ponteiro
```

## Windows: latente, não ativo

`GetSystemMetrics` devolve coordenadas **lógicas** num processo que não
declara DPI awareness — e binário Rust sem manifest é DPI-unaware. O
DXGI captura em físico. Fora de 100% de escala, `input_win::mover`
estaria dividindo físico por lógico e errando pela escala.

Medido em 04/11/2026 com `windows/sondas/ler-cursor.ps1` (tarefa `/IT`,
porque pelo SSH se lê a sessão 0 e o número é inválido):

```
sem-dpi-aware: cursor=1258,1369 virtual=2560x1440
com-dpi-aware: cursor=1258,1369 virtual=2560x1440
```

Idênticos ⇒ a escala efetiva é 100% e o caminho está correto hoje.
(O `DpiValue=0x1` em `HKCU\Control Panel\Desktop\PerMonitorSettings` é
de um monitor que não está em uso — o registro sugeriu escala e a
medida desmentiu.)

**Se mudar a escala do Windows, isto quebra.** O conserto é declarar
`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` na partida do
host, o que faz `GetSystemMetrics` devolver físico e torna a suposição
do código verdadeira. Não foi feito agora para não mudar, sem poder
testar, um caminho que está funcionando.

## Regra

Host novo, ou tela nova num host que já existe: **meça os dois espaços
antes de confiar que são um**. O teste é `--abs` no `input-send`, e o
caso que revela é o **meio** do quadro — não o canto, que o clamp
disfarça, e não o movimento relativo, que nem passa pela conversão.

```bash
logistream-input-send <host>:<porta+1> --abs 1680 1050   # meio do quadro
```

## Veja também

- [[../skills/verify-the-mac]] — a receita com os quatro pontos
- [[cursor-without-mouse]] — o outro problema de cursor, sem relação com este
- [[verify-what-arrived]] — por que `--rel` dizer "+200 exatos" não
  provava nada aqui
