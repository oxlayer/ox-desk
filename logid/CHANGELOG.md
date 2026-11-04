# Novidades

Uma entrada por coisa que mudou e que dá para notar usando. Vai embutida
no binário (`include_str!`), então o que o painel mostra é sempre o que
aquela máquina está rodando — não o que o repositório tem.

Formato: `## <data> · <título>` e uma linha de descrição.

## 2026-11-04 · "Ver a tela" do Mac já usa o nosso, não RDP
Clicar acorda o host da outra máquina sozinho e abre nossa janela. A
imagem foi corrigida (ela era redimensionada errado), ganhou taxa de
bits de LAN e keyframe a cada 2s para o erro cicatrizar. O Windows
segue em RDP até existir captura lá.

## 2026-11-04 · Tela remota própria: teclado e mouse já voltam
O laço fechou — o input do cliente chega ao host e é aplicado. Falta
trocar o botão "Ver a tela" para chamar o nosso em vez do RDP.

## 2026-11-04 · Tela remota própria: já dá para VER o Ubuntu no Mac
A imagem é capturada, codificada pela GPU, atravessa a rede e é
decodificada no M1 a 72 fps. Falta mandar teclado e mouse de volta para
poder usar, e não só olhar.

## 2026-11-04 · Tela remota própria: a imagem já chega no Mac
Capturada no Ubuntu, codificada pela GPU e transmitida por QUIC: 77 fps
a 1440p, 1,7 Mbps, sem perder quadro. Falta desenhar na tela e mandar o
teclado e o mouse de volta.

## 2026-11-04 · Tela remota própria: primeira metade pronta
Captura do Wayland e codificação por GPU funcionando: 1440p a 75 fps em
6,4 ms por quadro, 5,5 Mbps. Falta o transporte e o cliente do Mac.

## 2026-11-04 · Borda desligada, teclado vira o líder
Os dispositivos trocavam de máquina sozinhos. A borda saiu de cena e a
troca passa a ser o botão Easy-Switch; o reconciliador agora segue o
teclado, não o mouse, para não desfazer o que você acabou de fazer.

## 2026-11-04 · Versão do build no rodapé
Número incremental e commit, iguais nas três máquinas quando estão em
dia — hash sozinho diz que estão diferentes, não qual está atrás.

## 2026-11-04 · Gerenciador de tarefas
Processos das três máquinas, ordenados por CPU, com encerrar em dois
passos. A unidade de CPU é declarada: percentual no Unix, segundos
acumulados no Windows.

## 2026-11-04 · Disposição das máquinas na grade
Arraste cada máquina para onde ela fica na mesa; as bordas saem daí.
Vizinho que não consegue receber dispositivo aparece riscado, com o
motivo.

## 2026-11-04 · Windows parou de apagar o daemon
O Defender removia o `logid.exe` como ameaça. O binário agora chega
assinado, com carimbo de tempo.

## 2026-11-03 · Tela remota nativa de cada SO
Botão "Ver a tela" no painel, com opções de teclado+mouse, som e
microfone — cada uma desabilitada quando o cliente não sabe fazer.

## 2026-11-03 · Navegador de arquivos
Lista pastas das três máquinas, local ou por SFTP. Só leitura.

## 2026-11-03 · O Mac entrou
Terceira máquina no painel: status, arquivos e tela. Sem receptor
Logitech, ele ainda não recebe teclado e mouse.

## 2026-11-03 · Painel e daemon no Windows
Antes só existia no Ubuntu. Agora é o mesmo binário nos dois, com ícone
no Menu Iniciar e na bandeja.
