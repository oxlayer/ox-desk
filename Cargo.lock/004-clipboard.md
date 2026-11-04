# 004 — Clipboard

## O que já funciona

GSConnect ↔ KDE Connect, pareados e com TLS. Verificado:
`Paired: true`, `Connected: true`,
`EncryptionInfo: 'Verification key: 4BCF0AF8'`.

O clipboard do **Deskflow não funciona no Wayland** aqui: precisa de
`libportal` ≥ 0.10 e o Ubuntu 26.04 traz 0.9.1. Por isso o transporte é
o KDE Connect.

## Política é o produto, protocolo não

O protocolo está resolvido. O que falta é **política**, e é aí que o
nosso sistema agrega:

| Chave GSConnect | Valor aqui | Razão |
|---|---|---|
| `receive-content` | `true` | clipboard do Windows chega aqui |
| `send-content` | `false` | o clipboard **daqui não sai** |

Assimétrico de propósito: o Bitwarden roda nesta máquina. Sync
automático nas duas direções espelharia senha copiada para a outra
máquina, onde fica até ser sobrescrita.

### O que queremos além disso

- **Lista de bloqueio por app de origem.** Se o foco estava no
  Bitwarden, não propagar — nem com sync ligado. É a política que o
  KDE Connect não tem.
- **TTL.** Clipboard recebido expira em N segundos.
- **Push manual explícito** sempre disponível, independente do auto.
- **Histórico local opcional**, nunca sincronizado.

## Armadilhas registradas

**`gsettings set` não persiste** nessas chaves: reportava `true` no
`get` enquanto `dconf read` voltava vazio e `clipboard/` não aparecia
em `dconf list`. Sem explicação limpa. Use `dconf write` e confira com
`dconf read`.

**Testar por SSH não funciona.** `Set-Clipboard` via SSH escreve num
clipboard que o `kdeconnectd` da sessão interativa não vê — e o
`Get-Clipboard` da mesma sessão SSH devolve o valor, o que deixa o teste
enganosamente verde. Teste de clipboard tem que rodar **no agente**,
dentro da sessão gráfica.

**Firewall.** O instalador do KDE Connect **não cria regra** no
Windows. Ele escutava 1716 em TCP/UDP e `Get-NetFirewallRule -DisplayName
"*KDE*"` não devolvia nada. Regras 1716-1764 TCP+UDP são parte do
bootstrap ([`002`](002-ssh.md)).
