# Novidades

Uma entrada por coisa que mudou e que dá para notar usando. Vai embutida
no binário (`include_str!`), então o que o painel mostra é sempre o que
aquela máquina está rodando — não o que o repositório tem. Tradução de
`CHANGELOG.md`: as duas têm as mesmas entradas, nas mesmas datas, e um teste
confere.

Formato: `## <data> · <título>` e uma linha de descrição.

## 2026-11-16 · O cursor some enquanto o controle está na outra máquina
Enquanto o teclado e o mouse estavam na outra máquina, o cursor do Windows
ficava parado no meio da tela. Agora ele some, e volta onde o controle
volta. Nada rouba o foco para isso: um menu aberto ou um campo pela metade
continuam lá quando você volta.

## 2026-11-16 · A borda segue os seus monitores
Com monitores de tamanhos diferentes lado a lado, parte da borda de uma
tela não trocava de máquina, e a volta podia pôr o cursor onde não há
tela. Agora a borda é calculada por monitor, e o cursor volta para um
monitor de verdade, na altura em que saiu.

## 2026-11-15 · Páginas da web não controlam mais o OxDesk
A API local respondia a qualquer página aberta num navegador do mesmo
computador, e uma página poderia ter mandado o seu teclado para outra
máquina ou mudado as configurações. Agora ela só aceita comandos do
próprio painel, da bandeja e da linha de comando.

## 2026-11-15 · Ir para lá é tão rápido quanto voltar
Cruzar a borda em direção à outra máquina agora leva cerca de 50 ms em vez
de 300 a 450: o outro lado é acordado enquanto o mouse ainda está a caminho
da borda, então ao chegar não há mais nada a esperar.

## 2026-11-15 · De volta pela borda da tela, e troca em meio segundo
Com a borda ligada, empurrar o mouse além da borda da outra tela que fica
do lado desta máquina agora traz o teclado e o mouse de volta -- um empurrão
firme, para a barra de rolagem nessa borda não disparar. O cursor entra na
mesma altura em que saiu. E a troca leva cerca de meio segundo em vez de
cinco: o daemon não pergunta mais à outra máquina, por SSH, o que já sabe.

## 2026-11-15 · O Windows manda o teclado e o mouse para outra máquina
Com o receptor Logitech no Windows, os dois Ctrl, a borda da tela e
"Mandar para" no painel ou na bandeja agora levam o teclado e o mouse para
a outra máquina pela rede, como o Linux sempre fez. Os dois Ctrl trazem
de volta, mesmo com a rede fora. Antes disso, a única saída do Windows
era o botão Easy-Switch do próprio dispositivo.

## 2026-11-15 · Escolha as teclas do atalho
Configurações › Troca de máquina: os dois Ctrl, os dois Shift ou os dois
Alt. As mesmas teclas mandam o teclado e o mouse e os trazem de volta, e
uma dica na Visão geral diz quais estão em uso: o gesto deixou de ser
segredo.

## 2026-11-15 · A borda liga sem reiniciar, e diz o que está fazendo
Ligar a borda no painel exigia reiniciar o daemon, e ninguém era avisado.
Agora ela começa em até um segundo, e as Configurações mostram o estado em
palavras: observando, aguardando porque o teclado e o mouse estão em outra
máquina, ou por que ela não pode rodar aqui.

## 2026-11-15 · A tela remota do Ubuntu segue o monitor que você usa
O host do Ubuntu gravava um conector de monitor fixo. Quando o monitor
mudou de porta, ele passou a gravar uma tela virtual invisível, e o mouse
mandado de outra máquina andava lá. Agora ele pergunta ao GNOME qual é o
monitor principal.

## 2026-11-04 · A imagem não congela mais até você mexer o mouse
O laço da janela bloqueava esperando evento do sistema, e a thread de
rede enchia o slot com o quadro novo sem acordar ninguém — então a
imagem só atualizava quando você mexia o mouse ou digitava. O relato
("congela numa mensagem do chat até eu mexer o mouse") descreveu o
mecanismo exato. Agora a rede acorda a janela a cada quadro: medido, 720
quadros desenhados a 59 fps com ninguém tocando em nada.

## 2026-11-04 · Input do Mac para o Ubuntu: tecla presa e lentidão
Cada evento era uma chamada ao compositor **esperando resposta**, na
mesma thread que bombeia o vídeo — então a tecla esperava o quadro e o
quadro esperava a tecla. Agora a injeção não espera resposta (ela não
tem nada de útil). O host do Ubuntu também passou a respeitar 60 fps em
vez de mandar 80: medido, 4,5 Mbps contra 6,2, sem diferença visível numa
tela de 60 Hz. E o visualizador do Mac copia os pixels por linha em vez
de um a um — eram 921 mil iterações por quadro para produzir bytes que já
vinham no formato certo.

## 2026-11-04 · O RDP saiu: o Windows também assiste pelo nosso protocolo
O visualizador passou a compilar lá, decodificando por NVDEC — não existe
`libavcodec` no Windows e era só isso que mantinha aquele caminho no
mstsc. Os quatro trajetos entre as três máquinas agora são nossos.
Medido daqui com a GPU: 840 quadros a 65 fps, zero perdido.

## 2026-11-04 · Decodificação pela GPU, sem depender do ffmpeg
O NVDEC decodifica o nosso fluxo: 2560x1440 a 250 fps, 4 ms por quadro
contando a volta para a CPU. A conversão de cor foi conferida contra o
ffmpeg no mesmo quadro (média R=60 G=60 B=61 contra R=60 G=59 B=61 — um
ponto de arredondamento). É o que vai liberar o visualizador no Windows,
onde não existe libavcodec, e com ele o último RDP do sistema.

## 2026-11-04 · O painel pode perguntar ao dispositivo onde ficam os canais
O comando `hosts <slot>` lê os hosts guardados no próprio teclado (0x1815),
com o nome e o tipo de rádio de cada canal, unifying ou bluetooth. Isso
encerrou uma
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
No estilo do app do Windows que serviu de referência: lateral translúcida com as
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
O Defender removia o executável do daemon como ameaça. O binário agora chega
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
