# Os caminhos de tela

Quatro combinações de quem assiste e quem é assistido. Desde o NVDEC
(04/11) **todos são nossos** — nenhum passa por RDP ou VNC, exceto o
Mac como host, que ainda não produz quadro.

| De (assiste) | Para (host) | Captura | Codifica | Decodifica |
|---|---|---|---|---|
| Ubuntu | Windows | DXGI Desktop Duplication | NVENC | NVDEC |
| Windows | Ubuntu | PipeWire via Mutter ScreenCast | NVENC | NVDEC |
| Mac | Windows | DXGI | NVENC | ffmpeg (`swscale`) |
| Mac | Ubuntu | PipeWire | NVENC | ffmpeg |
| Ubuntu/Windows | Mac | avfoundation | VideoToolbox | NVDEC / ffmpeg |

O host do Mac sobe por **LaunchAgent**, não por SSH: a captura depende
de permissão TCC concedida a quem lançou, e por SSH o AVFoundation
trava em vez de falhar ([[../.claude/rules/sessao-grafica]]).

O Mac decodifica por ffmpeg porque não tem NVDEC. Isso deixou de ser
detalhe quando virou **grupo de controle**: o caminho do Mac usava
`swscale` (bilinear) e estava ótimo enquanto o caminho pela GPU, com
amostragem por vizinho mais próximo, serrilhava cada letra. Comparar os
dois foi o que apontou o bug — ver [[../specs/010-remote-desktop.md]].

## Transporte

QUIC (`quinn`): vídeo por **datagrama** (perder quadro é melhor que
atrasar), input por **stream** (não pode perder tecla nem reordenar).

O host serve **um cliente por vez**. Um segundo `logistream-sink`
tentando medir enquanto alguém assiste leva timeout — não é bug, é a
fila de accept.

## Input

O protocolo carrega keycode de **evdev** em todos os caminhos, porque é
o que `NotifyKeyboardKeycode` do Mutter recebe; `scancode_de_evdev()`
traduz para Set-1 no Windows.

Dois tipos de movimento de ponteiro:

- `Mouse { x, y }` — absoluto, no espaço do stream. O visualizador usa
  este: ele sabe onde o cursor está dentro da janela.
- `MouseRel { dx, dy }` — relativo. O KVM por LAN usa este, porque jogo
  lê movimento relativo e com absoluto a mira trava nas bordas.

## Veja também

- [[maquinas]]
- [[../specs/010-remote-desktop.md]] — o stack completo
- [[../specs/005-input-switch.md]] — troca de máquina e KVM

## Ubuntu sem monitor ligado

Com as saídas em `disconnected`, o `RecordMonitor` responde `Unknown
monitor`. O host passa a pedir `RecordVirtual`, em 1920×1080, e o GNOME
cria o monitor `Meta-0` enquanto o host viver.

O stream virtual renegocia o formato logo depois de nascer, com o mesmo
tamanho. Trocar o `Codificador` nessa hora dava SIGSEGV (saída 139), por
isso o host mantém o que já tem.

Medido em 07/11: 25 quadros em 10 s, zero perdidos, com a tela parada.
O Mutter só entrega quadro quando algo muda.
