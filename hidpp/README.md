# `hidpp`

HID++ 2.0 sobre HID cru. Usado pelo [`logid`](../logid/) (Linux) e pelo
[`agent`](../agent/) (Windows) — **uma cópia só** do protocolo.

Substituiu o backend via `solaar` no Linux. Medido:

| | solaar | HID++ direto |
|---|---|---|
| presença de um dispositivo | 5,9 a 15,3 s | **22 a 81 ms** |
| inventário completo | 5,9 a 15,3 s | ~1,5 s |
| troca de canal, ponta a ponta | 105 s | ~9 s |

## Diagnóstico

```bash
cargo run --release --bin probe
```

Lista endpoints, abre o receptor e varre os slots com nome, canal e ping.
Útil para descobrir o **nome exato** que a config precisa: o HID++ reporta
`MX Keys Wireless Keyboard`, enquanto o solaar dizia `MX Keys Keyboard`.

## Decisões que custaram medição

**Report longo na coleção usage 2.** O receptor expõe uma coleção HID por
tipo de report (1 = curto `0x10`, 2 = longo `0x11`, 4 = muito longo).
Escrever o tamanho errado na coleção errada falha com `hidapi error:`
vazio. Usamos sempre o longo, para escrever e ler na mesma coleção.

**Endereçamos o receptor, não o nó do dispositivo.** No Linux cada
dispositivo pareado tem nó hidraw próprio, mas ele atende no **índice de
slot do receptor** (medido: `/dev/hidraw2` responde em `idx=1`), não em
`0xFF`. Como o nó do receptor atende todos os slots, é ele que abrimos.

**Timeout de ping separado (250 ms) do de chamada (700 ms).** Um
dispositivo presente responde em 26-90 ms; varredura de 6 slots vazios
caiu de 4,4 s para 1,5 s.

**Ausência de resposta no `set_host` é o caso esperado.** O dispositivo
troca de canal e o rádio cai antes da resposta. Tratar isso como erro
fazia o sistema reportar falha numa troca bem-sucedida.
