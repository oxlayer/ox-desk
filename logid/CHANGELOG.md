# Novidades

Uma entrada por coisa que mudou e que dá para notar usando. Vai embutida
no binário (`include_str!`), então o que o painel mostra é sempre o que
aquela máquina está rodando — não o que o repositório tem.

Formato: `## <data> · <título>` e uma linha de descrição.

## 2026-11-04 · O painel pode perguntar ao dispositivo onde ficam os canais
`logid hosts <slot>` lê os hosts do próprio teclado (0x1815), com nome e
tipo de rádio: canal 1 `ubuntu-host` (unifying), canal 2 `windows-host`
(unifying), canal 3 `mac-host` (bluetooth). Isso encerrou uma
hora de investigação errada: eu deduzia o pareamento pelo que os
receptores mostram, e receptor não sabe nomear dispositivo ausente. O
inventário agora também diz `paired`, separando "slot vazio" de "pareado,
mas o dispositivo está em outra máquina" — antes os dois eram o mesmo
JSON.

## 2026-11-04 · Botão para pausar a leitura dos dispositivos
Os dispositivos voltam ao canal 1 sozinhos, com borda e reconciliador
desligados nas três e nenhuma linha de log reivindicando. Eliminado todo
código nosso que troca canal, sobraram duas hipóteses: a nossa leitura
HID++ perturbando o rádio, ou o hardware. Em Disposição há agora um botão
que pausa a sondagem por 10 minutos — troque de máquina durante a pausa:
se ficar de pé, o problema é nosso; se voltar sozinho, é o rádio. Trocar
de canal continua funcionando durante a pausa.

## 2026-11-04 · Uptime, RAM e CPU em todos os cartões, sempre
A máquina que você está olhando ficava sem detalhe nenhum: a leitura ia
por SSH e ninguém faz SSH para si mesmo — daí você ver a RAM do MacBook
pelo Windows e não pelo próprio MacBook. Agora o cartão local lê
localmente. A RAM do Linux nunca tinha funcionado (os dois números saíam
colados por dependerem da ordem do /proc/meminfo) e o bug sobreviveu
porque esta é a única máquina Linux e o detalhe dela nunca era
consultado. Entrou CPU junto, e a linha aparece sempre — com travessão
quando não deu para ler, em vez de desaparecer.

## 2026-11-04 · Cursor no lugar certo e tela cheia que estica
Dois defeitos meus, do mesmo conserto anterior. O cálculo da posição do
mouse usava o tamanho da imagem como se fosse o da tela remota — numa
janela de 1280x720 contra 2560x1440, seu cursor só alcançava o quarto
superior esquerdo. E o desenho exigia casar o tamanho exato, então ao
entrar em tela cheia todo quadro era descartado e ficava na tela o
buffer antigo, pequeno, no canto. Agora a imagem se ajusta à janela sem
distorcer (com barras quando a proporção difere) e o cursor é mapeado
pela imagem, não pela janela.

## 2026-11-04 · Painel com barra lateral, favoritos e um painel por vez
No estilo que você apontou no app do Windows: lateral translúcida com as
máquinas em cima, favoritos logo abaixo e os painéis listados — o
conteúdo troca no lugar em vez de ser um rolo único de cinco seções. A
estrela fixa uma máquina em Favoritos; o ▶ ao lado dela abre a tela
direto. Em janela estreita a lateral vira uma barra horizontal.

## 2026-11-04 · "Ver a tela" do Windows, daqui, já é o nosso
O visualizador passou a compilar no Linux (libavcodec do apt), então o
botão do painel do Ubuntu deixou de chamar RDP. Três defeitos saíram no
caminho: reconectar era recusado, um handshake que falhava derrubava o
host inteiro, e o primeiro quadro de uma reconexão às vezes vinha sem
ser quadro-chave — a janela abria preta reclamando do decodificador.
Agora nada sai antes de um quadro-chave, por garantia e não por sorte.

## 2026-11-04 · A borda do Windows também obedece ao interruptor
Eu havia corrigido só a do GNOME. O Windows tem implementação própria, e
o log provou o estrago: dois segundos depois de desligar pela tela, ela
se armou e devolveu mouse e teclado para o Ubuntu. Agora as duas leem a
flag a cada volta — e o dead-man's switch, que devolvia os dispositivos
se você não digitasse em 25s, para junto.

## 2026-11-04 · O Windows serve a própria tela pelo nosso protocolo
Captura por DXGI, codificação na GPU, QUIC — o mesmo caminho do Ubuntu.
Verificado daqui: 2560x1440, 174 quadros em 10s, zero perdido. No painel
do Mac, "Ver a tela" do Windows já é o nosso, não RDP. No painel do
Ubuntu continua RDP, porque o visualizador só compila no macOS por
enquanto — e trocar um botão que funciona por um que diz "sem cliente"
não seria progresso.

## 2026-11-04 · O painel mostra a borda e o reconciliador de cada máquina
A borda é um comportamento de par: a do Windows apontada para cá
devolvia mouse e teclado a cada encostada, enquanto esta tela dizia —
com razão — que a borda daqui estava desligada. Era invisível sem abrir
o painel da outra máquina. Agora cada cartão mostra o estado dos dois
interruptores daquele host, em âmbar quando ligados.

## 2026-11-04 · Os interruptores de borda e reconciliador agora desligam
Desligar a borda pela tela não desligava nada: as barreiras eram criadas
no boot e a flag nunca era lida de novo. O reconciliador, por sua vez,
pedia ao outro computador que devolvesse o mouse, acreditava sem
conferir, e repetia — sete vezes em dois minutos. Agora ele confere se
o dispositivo saiu mesmo e, em três tentativas sem efeito, se desliga e
diz por quê no log.

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
