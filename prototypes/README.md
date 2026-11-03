# Protótipos

Código de investigação que **provou** um caminho antes de virar Rust.
Guardado porque o valor está no que ele estabeleceu, não no código.

| Arquivo | O que provou |
|---|---|
| [`border-detect.py`](border-detect.py) | `org.gnome.Mutter.InputCapture` entrega evento de borda e devolve o ponteiro, sem libei — enquanto o portal equivalente aceita `CreateSession` e nunca responde. Ver [`../specs/005`](../specs/005-input-switch.md#detecção-de-borda-mutter-direto-não-o-portal). |

Rodar: `/usr/bin/python3 border-detect.py` — precisa do `python3-gi` e do
python do **sistema** (o do linuxbrew não tem `gi`).
