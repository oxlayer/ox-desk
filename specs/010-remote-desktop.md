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
| **v1** | Ubuntu → tela capturada, codificada em NVENC, gravada em arquivo | captura no Wayland e NVENC funcionam mesmo |
| **v2** | v1 por QUIC até o Mac, decodificada e exibida | transporte e decodificação |
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
