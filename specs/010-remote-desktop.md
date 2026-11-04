# 010 — Tela remota própria (`logistream`)

Pedido do usuário, depois de ver a dependência do Windows App:
*"quero implementação própria baseada talvez no rustdesk, feita em rust
igual parsec"* — com **H.264 por GPU e UDP+FEC desde o começo**, e áudio,
pareamento e NAT depois.

## O tamanho disto, dito antes de começar

É a base de código de um RustDesk. Captura por SO, codificação por
hardware, transporte com perda controlada, injeção de input por SO,
decodificação e render no cliente, áudio, cripto e pareamento. **Semanas,
não uma noite.** Qualquer promessa diferente seria mentira, e o projeto
já pagou caro por silêncio otimista.

Enquanto não presta, o Windows App continua no Mac. Ele sai quando o
nosso o substituir de fato, não quando o nosso existir.

## O terreno, medido (04/11)

Isto não é escolha de gosto — foi levantado nas três máquinas:

| | Ubuntu | Windows | macOS |
|---|---|---|---|
| GPU | RTX 3060 | RTX 3050 | Apple Silicon |
| encoder | NVENC (`libnvidia-encode.so.1`, driver 580) | NVENC | — (é cliente) |
| decoder | NVDEC | NVDEC | VideoToolbox |
| captura | portal `ScreenCast` + PipeWire | DXGI Desktop Duplication | — |
| injeção | libei/EIS (o `border.rs` já conecta nele) | `SendInput` | — |
| libav dev | via linuxbrew, 62.28 (sem sudo) | — | via brew |

Os **dois hosts são NVIDIA**, o que simplifica: um caminho de
codificação, não três. O Mac é cliente, e decodifica com VideoToolbox.

> O ffmpeg do linuxbrew veio **sem nvenc** (`ffmpeg -encoders` lista só
> `libx264` e `h264_v4l2m2m`). Por isso a codificação fala com o NVENC
> **direto**, via `libnvidia-encode`, em vez de passar pelo libavcodec —
> que de todo jeito seria uma camada a mais no caminho quente.

## Arquitetura

### Transporte: QUIC, não UDP cru

UDP+FEC foi o pedido, e é o que o vídeo precisa — mas escrever UDP cru
significa escrever também handshake, cripto, controle de congestionamento
e remontagem. QUIC entrega os quatro e **é UDP**:

| necessidade | como |
|---|---|
| vídeo com perda tolerada | **datagramas** QUIC (não retransmitidos) |
| FEC | Reed-Solomon por cima dos datagramas |
| input e controle | **streams** QUIC (confiáveis, ordenados) |
| cripto e identidade | TLS 1.3, embutido |
| congestionamento | embutido (BBR/Cubic) |
| trocar de rede sem cair | migração de conexão, embutido |

Escrever isso à mão seria reimplementar QUIC pior. `quinn` é a crate.

A identidade do par vira o **certificado TLS**: pareamento é confiar numa
chave pública, que é o modelo que o usuário já tinha descrito — *"aparece
no outro computador para aceitar e salva na keychain"*.

### Camadas

```
captura ──> encoder ──> empacotador ──> quinn ──> rede
(portal/   (NVENC)     (FEC + RTP-ish)
 DXGI)

rede ──> quinn ──> desempacotador ──> decoder ──> render
                   (reconstrói FEC)   (VideoToolbox)  (wgpu/Metal)
                                   ^
                              input <── streams
```

Cada camada atrás de um trait, por um motivo concreto: o primeiro encoder
que funcionar não será o último, e trocar NVENC por VideoToolbox ou x264
não pode significar reescrever o pipeline.

### O que NÃO vamos escrever

- **Codec.** H.264 vem do hardware.
- **Crypto.** TLS do QUIC.
- **Travessia de NAT na v1.** LAN primeiro; depois `quinn` + ICE.

## Etapas, com o que cada uma prova

| v | entrega | a incógnita que ela mata |
|---|---|---|
| **v1** | ✅ **FEITA** — tela capturada, NVENC, arquivo | captura no Wayland e NVENC funcionam mesmo |
| **v2** | ✅ **FEITA** — chega, decodifica e desenha no Mac | transporte e decodificação |
| **v3** | input do Mac de volta (libei no Ubuntu) | o laço fecha: dá para usar |
| **v4** | FEC, controle de taxa, Windows como host (DXGI) | qualidade e a segunda plataforma |
| **v5** | áudio, pareamento com PIN, NAT | o resto |

Cada etapa é usável ou descartável sozinha. Nenhuma depende de a
seguinte existir.

## Onde mora

Crate nova `logistream/`, separada do `logid`. O daemon continua sendo
control plane — status, arquivos, processos, troca de teclado — e não
vira servidor de vídeo. O `logid` aprende a **lançar** e **parear** o
`logistream`, como já lança o cliente de tela hoje.

## O Parsec Virtual Display Adapter não é lixo a varrer

O usuário quer apagar o Parsec quando o nosso funcionar. Justo — mas
aquele adaptador de vídeo virtual que ele instala tem função: dá à
máquina uma **tela para capturar quando não há monitor ligado**, e
permite resolução do cliente em vez da do host.

Se o Windows dali às vezes fica sem monitor, apagar o adaptador nos tira
uma capacidade que teríamos de reconstruir (driver de display virtual é
coisa séria, assinada, em kernel). A decisão fica para quando a v4
estiver de pé e dê para medir se faz falta — não para a hora da
arrumação.

## v1, metade feita: o NVENC está provado (04/11)

`cargo run --bin nvenc-probe` codifica quadros **sintéticos** — captura é
outro problema, e misturar os dois faria o fracasso de um parecer o do
outro.

```
NVENC carregado (dlopen de libnvidia-encode)
120 quadros 1920x1080 em 0.77s = 155 fps
por quadro: p50 3.56ms  p95 3.83ms
39.3 Mbps a 60fps
```

E o `ffprobe` lê o arquivo: `h264`, perfil `High`, `1920x1080`,
`yuv420p`, **120 quadros** — os mesmos que entraram.

Três coisas que isso resolve:

1. **Sem SDK e sem sudo.** A crate `moq-nvenc` faz `dlopen` da
   `libnvidia-encode`, que já veio com o driver. Nada a instalar, e numa
   máquina sem GPU NVIDIA a falha é ao *abrir*, não ao compilar — que é o
   comportamento certo para um binário que roda em três máquinas.
2. **3,56 ms por quadro** deixa espaço de sobra num orçamento de 16 ms
   (60 fps). O gargalo não vai ser a codificação.
3. **39 Mbps é o pior caso**: o gradiente sintético muda cada pixel a
   cada quadro, de propósito. Tela de verdade — texto parado, janelas
   paradas — comprime uma ordem de grandeza melhor.

### Um erro que o teste pegou e que teria mordido em produção

A primeira versão escreveu um arquivo que o `ffprobe` recusou:
`non-existing PPS 0 referenced`. O NVENC não repete SPS/PPS por padrão.

Num arquivo isso é chato; **numa transmissão é fatal**, porque o caso
normal da tela remota é o cliente entrar no meio — e sem os cabeçalhos
ele não tem como começar a desenhar nada. A config agora liga
`repeatSPSPPS`, que os reenvia em cada IDR.

Já que havia config explícita, entrou junto o `NV_ENC_TUNING_INFO_LOW_LATENCY`:
tela remota vive de latência, não de bitrate bonito.

### O que falta na v1: captura

A captura no Wayland precisa de `libpipewire-0.3-dev`, que **não está
instalada** e não existe no linuxbrew — é o primeiro `sudo apt` que este
projeto vai pedir.

Alternativa sem sudo: começar pelo **Windows como host** (DXGI Desktop
Duplication não precisa de biblioteca extra). Fica a escolher.

## v1 fechada: tela real, 1440p, em 38% do orçamento (04/11)

`cargo run --release --bin v1-record` captura o monitor, codifica em
NVENC e grava `.h264`:

```
formato: 2560x1440 VideoFormat::BGRx
180 quadros 2560x1440 em 2.38s = 75.5 fps
copia:  p50 1.05ms  p95 1.41ms
encode: p50 5.31ms  p95 5.40ms
total:  p50 6.38ms  p95 7.03ms  (orçamento 16.6ms a 60fps)
2.0 MB = 5.5 Mbps a 60fps
```

O `ffprobe` lê 180 quadros, `h264` High, `2560x1440`, e um quadro
extraído mostra a área de trabalho de verdade — VS Code, terminal, dock.

### O que os números dizem

**6,38 ms de 16,6 ms.** Sobram 10 ms para empacotar, cifrar e mandar. O
caminho quente não vai ser o gargalo; a rede vai.

**5,5 Mbps contra os 39 Mbps do gradiente sintético.** Era a previsão:
tela real tem texto parado e janela parada, e o encoder vive disso. O
sintético muda cada pixel de cada quadro de propósito, para medir o pior
caso — e o pior caso ficou sete vezes acima do real.

**BGRx em toda a cadeia.** O Mutter entrega BGRx, o NVENC consome ARGB,
e na memória são a mesma ordem de bytes — a cópia é crua, 1,05 ms, sem
nenhuma conversão de cor na CPU. Se tivesse havido conversão, ela
sozinha comeria o orçamento.

### A captura não passa pelo portal

`org.gnome.Mutter.ScreenCast`, a API privada que o próprio portal usa por
baixo. O portal `ScreenCast` exige **aprovação interativa** — abriria um
diálogo na tela da máquina capturada, que é exatamente a máquina onde
pode não haver ninguém sentado.

Mesmo precedente da borda, que fala com `Mutter.InputCapture` direto
porque o portal equivalente aceita `CreateSession` e nunca responde.

### Falta para a v2

Transporte já tem esqueleto e teste (`net.rs`: fatiamento, remontagem,
fora de ordem, quadro incompleto contado). Falta ligar as pontas e
escrever o cliente do Mac — decodificar com VideoToolbox e desenhar.

## v2, metade feita: a tela chega ao Mac (04/11)

`host` no Ubuntu, `sink` no Mac, pela LAN:

```
770 quadros inteiros em 10.0s = 77.0 fps
1989 datagramas, 1.7 Mbps
quadros perdidos (chegaram incompletos): 0
```

O arquivo que chegou lá tem 770 quadros `h264` High `2560x1440`, e um
quadro extraído mostra a área de trabalho do Ubuntu. Em loopback, onde o
relógio é o mesmo, a latência **captura → quadro remontado** é
**p50 6,6 ms**.

### Dois bugs, e o segundo era invisível

**Família de endereço.** `ubuntu-host.local` resolve em IPv4 e IPv6, o
Mac devolve o IPv6 primeiro, e o socket do cliente estava preso em
`0.0.0.0` — o quinn recusa com *"invalid remote address"*. O socket local
agora nasce da família do destino. É a terceira vez que nome com duas
famílias morde este projeto (ver [`002`](002-ssh.md)).

**O driver da conexão não rodava.** Este foi feio: o host anunciava
*"80 fps, 0 recusados"* e o receptor via **zero**. `send_datagram` apenas
**enfileira**; quem põe no fio é o driver da conexão, que é uma task do
tokio. O laço de envio bloqueava a thread em `recv_timeout` de um canal
da `std`, e numa runtime `current_thread` isso impede o driver de rodar.

Dois aprendizados ficam: o canal entre captura e rede é do **tokio**, e a
falha de `send_datagram` deixou de ser engolida — foi o `break` mudo que
fez o host reportar sucesso enquanto nada saía. Mesmo padrão do
`langfuse` e da `backfill_gate` no outro repo: o no-op silencioso é pior
que o erro barulhento.

### A medição de latência entre máquinas está errada, e isso é dito

Entre Ubuntu e Mac o número sai `0,0 ms` porque os relógios diferem e a
subtração satura. O rótulo avisa. Medir de verdade exige ida-e-volta
(o receptor devolve o carimbo e o host calcula), e isso entra junto com
o canal de input da v3 — que precisa do caminho de volta de qualquer
forma.

### Falta para fechar a v2

Decodificar com VideoToolbox e desenhar numa janela no Mac. O transporte
já entrega o quadro inteiro, em ordem, sem perda.

## v2 fechada: decodifica no M1 (04/11)

`view` no Mac contra o `host` no Ubuntu:

```
1080 quadros decodificados em 15s  (~72 fps)
host: 79 fps, 1.5 Mbps, 0 recusados
```

Decodificação por `libavcodec` do brew, conversão de cor por `swscale`,
janela por `winit` + `softbuffer`.

### Decisões que valem explicar

**Rede e decodificação numa thread, desenho na outra.** O laço de janela
do macOS é dono da thread principal e não pode bloquear; decodificar ali
travaria a interface a cada quadro.

**Entre as duas há um slot, não uma fila.** Quadro novo sobrescreve o que
ainda não foi desenhado. Fila só acumularia atraso — e numa tela remota
quadro velho não vale nada, já passou. Mesma razão do canal raso do lado
do host.

**A janela acorda por geração, não por timer.** `ControlFlow::Wait` mais
um contador que sobe a cada quadro: sem isso a interface gastaria CPU
perguntando se chegou algo.

**`ffmpeg-next` está amarrado ao macOS na config.** As duas máquinas têm
versões diferentes de libavcodec pelo brew (61 no Mac, 62 aqui), e
amarrar por plataforma resolve sem fingir que são a mesma — o host Linux
não decodifica nada.

### O que ainda é trabalho de CPU

A conversão YUV→BGRA no `swscale` e a cópia para o buffer da janela. Em
1440p isso é alguns milissegundos por quadro que o **VideoToolbox** (e um
shader de YUV no `wgpu`) eliminam. Está na v4, junto com FEC e controle
de taxa — primeiro fazer funcionar, depois fazer render direto.

### Falta para substituir o RDP

A v3: mandar teclado e mouse do Mac de volta para o host. Sem isso dá
para **ver** a máquina, não para usá-la.
