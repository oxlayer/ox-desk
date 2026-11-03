# 005 — Roteamento de input (o núcleo)

É a peça que não existe em open source. Ver
[`000`](000-overview.md#a-lacuna-que-justifica-o-projeto).

## A ideia

Quando o cursor encosta na borda (ou você aperta um atalho), há **dois
jeitos** de passar o controle para a outra máquina:

| Rota | Como | Latência | Quando |
|---|---|---|---|
| **física** | manda o mouse/teclado pular de canal Easy-Switch para o receptor da outra máquina (HID++ 0x1814) | nativa — é o rádio do próprio dispositivo | a outra máquina tem receptor com o device pareado |
| **LAN** | o Deskflow transmite eventos pela rede | ~1 ms + rede, e depende de portal/libei | a outra máquina não tem receptor, ou o canal não está pareado |

O Flow faz a primeira. O Deskflow faz a segunda. **Nenhum escolhe entre
as duas.** Escolher é o produto.

A rota física é melhor quando disponível: latência nativa, funciona com
a rede caindo, e não depende de injeção de input por portal — o que
**contorna o segfault do modo servidor do Deskflow no Linux**
([`README`](../README.md#ubuntu-como-servidor-testado-e-não-funciona-1270)).
Hoje as duas máquinas têm receptor Unifying, então a rota física é
possível nos dois sentidos.

## Detecção: tudo por portal, nada de hack

Os dois gatilhos têm API nativa no GNOME 50, e **verifiquei que o
backend declara as duas**:

```
/usr/share/xdg-desktop-portal/portals/gnome.portal:
  org.freedesktop.impl.portal.InputCapture      ← barreiras de ponteiro
  org.freedesktop.impl.portal.GlobalShortcuts   ← atalho global
```

- **Borda** → `org.freedesktop.portal.InputCapture` (v1). `CreateSession`
  foi aceito num teste direto por D-Bus; `SetPointerBarriers` + `Enable`
  dão o evento de encostar na borda. É o mesmo portal que a doc do
  Deskflow afirma que o GNOME não implementa — **afirmação desatualizada**.
- **Atalho** → `org.freedesktop.portal.GlobalShortcuts`, que funciona
  no Wayland sem gravar teclado.

Usar o InputCapture só para *detectar* a borda e então disparar uma
troca de canal HID++ é a sacada: a gente não precisa que ele seja bom
em *capturar e encaminhar* input, só em avisar que o cursor chegou na
borda.

## Pré-requisito: acesso ao hidraw

HID++ precisa abrir `/dev/hidraw*`. A regra do Solaar
(`/usr/lib/udev/rules.d/60-solaar.rules`) concede via `TAG+="uaccess"`,
que vira ACL para o usuário logado.

**Pegadinha já paga:** os `hidraw` nasceram às 11:37 e o Solaar foi
instalado às 13:22 — a regra não existia na criação dos nós, nenhuma ACL
foi aplicada, e o `solaar config` morria em `No supported device found`
com `getfacl /dev/hidraw0` mostrando só `user::rw-` (root).

```bash
sudo udevadm trigger --subsystem-match=hidraw --action=add   # ou religar o receptor
getfacl /dev/hidraw0 | grep user                           # confirmar a ACL
```

O `logid` deve **checar isso no startup** e dizer na API/GUI quando
faltar, em vez de falhar obscuro na hora da troca.

Mapeamento nesta máquina:

| hidraw | dispositivo |
|---|---|
| `hidraw0` | `046D:C52B` — receptor Unifying |
| `hidraw2` | `046D:4090` — MX Keys |
| `hidraw3` | `046D:408A` — MX Anywhere 3 |

## Restrição descoberta: só quem detém o device pode empurrá-lo

Testado: com o mouse no canal do Windows, `solaar config 1` falha com
`no online device found matching '1'`, e `solaar show` lista os dois
devices como `offline`.

O motivo é físico: o comando HID++ sai do receptor por rádio até o
dispositivo. Se ele está ouvindo **outro** receptor, não há como
alcançá-lo. Ou seja:

> A troca de canal só pode ser emitida pela máquina que **está com o
> dispositivo naquele momento**.

Isso torna a arquitetura obrigatoriamente **simétrica**: o control
plane não executa a troca, ele **pede ao detentor atual** que execute.
Quem está sem o device não tem como trazê-lo de volta pela rota física.

### E isso cria um problema no Windows

O Solaar é **Linux-only**. Para o Windows poder devolver o dispositivo,
precisa de HID++ lá também. Opções, em ordem de preferência:

| Opção | Avaliação |
|---|---|
| **OpenLogi** (Rust, MIT/Apache, tem `.msi`) | já fala HID++ 0x2111/0x2201; checar se expõe 0x1814 e se dá para dirigir por CLI |
| **Nossa implementação** sobre raw HID (hidapi) | 0x1814 ChangeHost é chamada de feature simples — é o pedaço onde Rust ou Go se pagam de verdade |
| Options+ | fechado, sem API, briga pelo receptor. Descartado. |

> ⚠️ Enquanto o lado Windows não emitir ChangeHost, a rota física é **só
> de ida** (Ubuntu → Windows); a volta é pela rota LAN ou pelo botão
> físico. Aceitável na fase 3, mas tem que estar explícito na GUI —
> nunca deixar o usuário descobrir isso sem teclado.

## Decisão

```
POST /api/hosts/{id}/focus
```

```
alvo alcançável?  ──não──▶ recusa, explica (não deixa o usuário no escuro)
       │ sim
       ▼
alvo tem receptor com NOSSO device pareado, em canal conhecido?
       │ sim                              │ não
       ▼                                  ▼
 rota FÍSICA                        rota LAN
 HID++ 0x1814 ChangeHost            garante deskflow-client no alvo
 nos dois devices (mouse+teclado)   servidor aponta para o alvo
       │                                  │
       └────────────┬─────────────────────┘
                    ▼
        confirma: o alvo reporta input chegando?
        não em 3 s ──▶ ROLLBACK para o canal/rota anterior
```

### Rollback não é opcional

Trocar de canal **tira seu teclado e mouse da máquina atual**. Se o
alvo não confirmar, você fica sem input nos dois lados — o pior modo
de falha possível deste projeto. Portanto:

1. Antes de trocar, gravar canal e rota atuais.
2. Trocar.
3. O **agente do alvo** confirma que viu input (ele tem sessão
   gráfica; nós não podemos verificar de fora — ver
   [`002`](002-ssh.md#limite-estrutural-isolamento-de-sessão)).
4. Sem confirmação em 3 s, voltar ao canal anterior.
5. Se o rollback também falhar, gritar: notificação, log, e a GUI em
   vermelho com a instrução manual (apertar o botão Easy-Switch).

Isso é a mesma regra de "disparo é destrutivo" do outro repo: a ação é
irreversível na prática se você perder o input, então preview e
rollback são obrigatórios, não desejáveis.

## Config

```toml
[input]
prefer = "physical"        # "physical" | "lan" | "auto"
confirm_timeout_ms = 3000

[input.border]
enabled = true
edge = "left"              # borda desta máquina que leva ao Windows
hold_ms = 250              # evita troca acidental ao passar raspando

[input.shortcut]
enabled = true
accel = "<Super><Alt>Right"

[[devices]]
name = "MX Anywhere 3"
hidraw_id = "046D:408A"
hosts = { "ubuntu-host" = 1, "windows-host" = 2 }   # canal Easy-Switch

[[devices]]
name = "MX Keys"
hidraw_id = "046D:4090"
hosts = { "ubuntu-host" = 1, "windows-host" = 2 }
```

O mapa `hosts` é o que o Solaar não tem e o Flow guarda na nuvem.
Aqui mora num TOML seu.

## Em aberto

- Enumerar os canais de cada device (`solaar show` depois da ACL
  resolvida) para preencher `hosts` sozinho.
- Latência real da troca HID++ ponta a ponta — medir antes de prometer.
- `InputCapture` v1 tem limitações que a v2 resolve (KDE 6.7.1+ tem v2,
  GNOME 50 tem v1). Descobrir se v1 basta para só *detectar* borda.
- E se o mouse estiver no canal da máquina desligada? Precisa de
  caminho de recuperação que não dependa de input.
