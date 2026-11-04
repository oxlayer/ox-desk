# Windows sem mouse não tem cursor — e isso quebra o KVM de duas formas

Máquina Windows sem **nenhum** dispositivo apontador conectado esconde
o cursor por completo. Medido:

```powershell
Get-PnpDevice -Class Mouse | Where-Object Status -eq OK   # NENHUM
# e, na sessão interativa:
GetCursorInfo -> ok=True flags=0 hCursor=0 pos=1356,728
```

`flags=0` é escondido, `hCursor=0` é "não existe cursor". **A posição
continua correta** — por isso o hover responde e nada aparece, que é
exatamente como o sintoma chega: *"vejo o mouse funcionar no hover mas
o cursor não aparece"*.

É o estado normal de uma máquina dirigida só por KVM: o dongle fica na
outra ponta.

## Consequência 1: o DXGI não tem o que entregar

`AcquireNextFrame` só reporta ponteiro quando existe um sendo
desenhado. Sem cursor, `LastMouseUpdateTime`, `PointerShapeBufferSize`
e `Visible` vêm zerados para sempre. Pedir a forma do cursor a quem não
tem cursor é circular — não adianta instrumentar o DXGI.

Resolvido desenhando uma seta própria (`cursor::seta_embutida`), com a
posição vinda de `GetCursorPos`, que funciona com cursor visível ou
não.

## Consequência 2: desenhar no stream não ajuda quem olha o monitor

E aqui mora a armadilha de verificação. A seta composta existe **dentro
do vídeo**. Quem está assistindo pelo visualizador vê; quem está
olhando o monitor físico da máquina **não vê nada**, porque ali o
Windows continua sem cursor.

Eu confirmei a correção decodificando o h264 e olhando o quadro — a
seta estava lá, no pixel certo — e reportei resolvido. O usuário estava
olhando o monitor. Verifiquei a coisa certa no lugar errado.

**Antes de declarar resolvido um problema visual, confira ONDE a pessoa
está olhando.** O log do host diz: sessão com `input conectado` e sem
nenhum `conectado:` de vídeo significa KVM sem visualizador — ou seja,
a pessoa está diante do monitor físico.

## O que o Parsec faz (e o que não funciona)

O Parsec documenta exatamente este problema. A solução deles é um
**driver de mouse USB virtual** próprio — um mouse que não se move,
só existe para o Windows voltar a desenhar o ponteiro. Não é genérico:
depende do driver deles.

Revelador: a documentação deles diz que o Parsec *"reproduz o cursor
escondido fielmente"*, ou seja, o cliente não vê ponteiro nenhum. Nossa
seta sintética faz mais que isso no visualizador.

**Mouse Keys não resolve** — o Parsec sugere, e medimos que não
funciona nesta máquina (Windows 10):

```
Flags no registro: 63      (MouseKeys LIGADO)
GetCursorInfo:     flags=0 hCursor=0
```

Também não existe chamada de API que resolva: as funções que deveriam
mostrar o cursor (`ShowCursor`) não têm efeito sem um mouse presente.
Por isso todo mundo cai em hardware ou driver.

## Correção de verdade: dar um mouse ao Windows

Qualquer dispositivo HID apontador faz o Windows voltar a desenhar o
cursor — inclusive um **receptor Unifying sem nada pareado**, que
enumera interfaces de mouse e teclado por conta própria.

Isso é melhor que a nossa seta em dois sentidos: aparece no monitor
físico, e o DXGI volta a mandar as formas de verdade (mão, barra de
texto, redimensionar), que passamos a desenhar no lugar da nossa.

A alternativa em código seria uma janela sobreposta, transparente a
cliques (`WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST |
WS_EX_NOACTIVATE`) seguindo o `GetCursorPos`. Funciona no monitor
físico e é capturada pelo DXGI junto, mas é Win32 a mais para manter e
convive mal com jogo em tela cheia exclusiva — que é justamente o caso
de uso do KVM nessa máquina.

## Veja também

- [[tela-bloqueada-windows]] — outra família de sintomas do mesmo host
- [[../../specs/005-input-switch.md]] — o KVM por LAN

## Desfecho (04/11)

O usuário plugou o dongle reserva no Windows. O resultado, medido:

```
Get-PnpDevice -Class Mouse  ->  HID-compliant mouse
host log                    ->  forma de ponteiro: Cor 32x32 pitch=128 (4096 bytes)
                                cursor desenhado em 1356,728 (32x32)
```

Um receptor Unifying **sem nada pareado** basta: ele enumera interfaces
de mouse e teclado sozinho. O Windows voltou a desenhar o cursor no
monitor físico, e o DXGI voltou a mandar a forma de verdade — colorida
com alfa, 32x32 —, que passamos a compor no lugar da nossa seta.

A seta embutida continua no código e continua certa: ela cobre quem
assiste uma máquina que não tem mouse nenhum. O que mudou é que ela
deixou de ser o caminho normal desta máquina.

Com cursor de sistema presente, `Visible` do DXGI passou a significar
algo, então voltamos a respeitá-lo — jogo em tela cheia esconde o
ponteiro de propósito, e desenhar um por cima seria intrometer-se.
Sem mouse, `Visible` é sempre falso e obedecê-lo apagaria justamente o
fallback.
