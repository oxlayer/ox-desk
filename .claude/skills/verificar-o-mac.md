# Verificar tela, mouse e teclado no Mac

Três coisas podem estar quebradas e dão o **mesmo** sintoma ("não
funcionou no Mac"): a permissão de Gravação de Tela, a de
Acessibilidade, e o host simplesmente não estar rodando. Esta receita
separa as três em ~1 min.

## 0. O host encerra sozinho — isso é de propósito

`RunAtLoad false` + saída por idle após 20 s sem cliente, para não
gastar bateria capturando para ninguém. **Dois testes meus falharam
por isso** antes de eu perceber: o `input-send` na mão pula o passo
que o `logid` faz sozinho.

Acordar é o que `acordar_host` faz (logid/src/screen.rs), e é
idempotente:

```bash
ssh mac 'pgrep -x logistream-host >/dev/null 2>&1 ||
         launchctl kickstart "gui/$(id -u)/ai.logi.logistream-host" >/dev/null 2>&1;
         sleep 1; pgrep -x logistream-host >/dev/null && echo vivo || echo morto'
```

Pela bandeja ou pelo painel não precisa: o `mandar` já chama isso antes
de abrir o KVM.

## 1. A tela abre?

```bash
ssh mac 'launchctl kickstart -k gui/$(id -u)/ai.logi.logistream-host; sleep 7
         tail -8 ~/.local/state/logistream-host.log'
```

Passar é ver as quatro linhas:

```
  avfoundation: tela 1, 3360x2100, UYVY422
  h264_videotoolbox pronto, 28 Mbps alvo
  escutando em 0.0.0.0:9777
  acessibilidade concedida: teclado e mouse serao injetados
```

Parar em `abrindo a tela 1...` = **falta Gravação de Tela** para este
binário. Sem `acessibilidade concedida` = falta Acessibilidade. Sem
TCC o AVFoundation **trava em vez de falhar**, então a última linha do
log vira uma mensagem do ffmpeg sobre formato de pixel — que não tem
nada a ver com o problema. Ver [[../rules/sessao-grafica]].

## 2. O vídeo chega?

```bash
cargo run --release --bin logistream-sink -- mac-host.local:9777 5
```

Esperado: ~46 fps, `quadros perdidos: 0`.

## 3. O mouse chega?

As sondas em `mac/sondas/` compilam em 1 s e existem porque **"parece
que não mexeu" não é medida**:

```bash
scp mac/sondas/*.swift mac:/tmp/
ssh mac 'cd /tmp && swiftc -O -o cursor cursor.swift && swiftc -O -o tecla tecla.swift'
```

```bash
echo "antes:  $(ssh mac /tmp/cursor)"
cargo run --release --bin logistream-input-send -- mac-host.local:9778 --rel 20
echo "depois: $(ssh mac /tmp/cursor)"
```

`--rel 20` = 20 × +10 px. Passar é X exatamente +200 e Y intacto.
Porta **9778**: vídeo e input são portas separadas (`porta + 1`).

## 3b. O mouse chega no lugar CERTO?

`--rel` não responde isso: ele acumula deltas e nunca passa pela
conversão de espaço. Ele já disse "+200 px exatos" com o caminho
absoluto quebrado — ver [[../rules/espaco-de-coordenadas]].

O caso que revela é o **meio** do quadro. O canto não serve: o sistema
prende o ponteiro na borda e o erro fica escondido atrás do clamp.

```bash
for p in "0 0" "3359 2099" "1680 1050" "840 525"; do
  ssh mac 'pgrep -x logistream-host >/dev/null || launchctl kickstart "gui/$(id -u)/ai.logi.logistream-host"; sleep 2'
  cargo run --release --bin logistream-input-send -- mac-host.local:9778 --abs $p
  echo "quadro ($p) -> cursor $(ssh mac /tmp/cursor)"
done
```

Esperado num Retina de 3360×2100 pixels / 1680×1050 pontos — metade de
tudo:

```
quadro (0 0)         -> cursor 0 0
quadro (3359 2099)   -> cursor 1679 1049
quadro (1680 1050)   -> cursor 840 525
quadro (840 525)     -> cursor 420 262
```

> **Acorde o host dentro do laço.** Sem isso ele morre por idle no meio
> da bateria de testes e as leituras seguintes repetem o último valor
> parado na tela — quatro linhas idênticas que parecem resultado e não
> são. Aconteceu comigo nesta mesma receita.

## 4. O teclado chega?

```bash
ssh mac '/tmp/tecla > /tmp/tecla.out 2>&1 &' & sleep 1
cargo run --release --bin logistream-input-send -- mac-host.local:9778 --segurar 30
sleep 4; ssh mac 'cat /tmp/tecla.out'
```

`A: PRESSIONADA` é passar. `30` é o evdev de A; a sonda olha
`kVK_ANSI_A = 0`, que é o código do macOS — a tradução é o que está
sendo testada.

## A permissão sobreviveu ao rebuild?

Essa é a pergunta que custou mais caro. Binário ad-hoc amarra o TCC no
cdhash, que muda a cada build; identidade estável amarra no designated
requirement, que não muda. Para provar:

```bash
ssh mac 'cd ~/logi/logistream && touch src/bin/host.rs &&
         export PATH=/opt/homebrew/bin:$PATH PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig &&
         ~/.cargo/bin/cargo build --release --bin logistream-host 2>&1 | tail -1 &&
         install -m755 target/release/logistream-host ~/.local/bin/ &&
         ~/.local/bin/assinar.sh ~/.local/bin/logistream-host'
```

Depois repetir o passo 1. Se abrir, sobreviveu.

**Não rode `criar-identidade.sh` de novo.** Ele recusa justamente por
isso: trocar o certificado invalida a concessão e cobra outra ida às
Configurações do Mac. Foi o que me aconteceu no meio deste trabalho.

## Quadro preto com contagem perfeita

Em 07/11 o host mandava 59 fps de preto em 1920×1440. O índice `1` tinha
virado a câmera do iPhone, porque o `avfoundation` conta câmeras antes
da tela. Desde então ele abre `Capture screen 0` pelo nome. Confira o
tamanho e a luminância do quadro, e não só a contagem.

Sobre o wizard dizer "falta" com a chave ligada: filho do `logid` responde
pelas permissões do `logid`. Por isso a pergunta vai como job do launchd.

## Veja também

- [[deploy]] — o passo `mac: assinar o host` falha se o binário ficar ad-hoc
- [[../rules/sessao-grafica]] — por que nada disso sobe por SSH direto
- [[../rules/verificar-o-que-chegou]] — por que o log do host é a prova
