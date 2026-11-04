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

## Implementado (04/11) — e o KDE Connect saiu

Clipboard de texto sincronizado nas três máquinas, sem terceiros.

### Linux: a API privada do Mutter

Nem `wl-clipboard` (o Wayland não deixa ler a seleção sem foco; o
`ext-data-control` não aparece nos globais deste compositor) nem o
portal `org.freedesktop.portal.Clipboard` (existe, mas amarrado a uma
sessão com diálogo de permissão).

A mesma `org.gnome.Mutter.RemoteDesktop` que já usamos para injetar
input entrega o clipboard inteiro — `EnableClipboard`,
`SelectionRead`, `SetSelection`, `SelectionWrite`, mais os sinais
`SelectionOwnerChanged` e `SelectionTransfer` — e a sessão **sobe sem
ScreenCast**, o que foi sondado antes de escrever qualquer coisa.

Duas pegadinhas, as duas encontradas sondando e as duas comentadas no
código:

1. **`EnableClipboard` com `mime-types` nos torna donos da seleção.**
   A leitura passa a falhar com *"Tried to read own selection"*. Para
   observar, chame sem tipos; declare tipos só ao publicar.
2. **O fd de `SelectionRead` vem não-bloqueante.** `read_to_end` nele
   devolve `WouldBlock` imediatamente — e quem engolir esse erro lê
   zero bytes e conclui que o clipboard está vazio. Foi exatamente o
   que a primeira sonda concluiu, com o clipboard cheio.

### Transporte: SSH, não porta nova

A API escuta só em `127.0.0.1` por desenho (specs/008). Clipboard
carrega senha com alguma frequência; abrir porta na LAN para ele seria
superfície nova para o dado mais sensível que passamos a mover.

SSH já é o canal confiável entre as máquinas. ~300 ms por cópia, que é
invisível no uso real: você copia, troca de máquina, e só então cola.

O caminho do binário remoto vem **por sistema**, não do PATH: o shell
não-interativo do SSH não carrega o PATH do login, e o Mac respondeu
`zsh:1: command not found: logid`. O campo `agent` da config também não
serve — aponta para outro executável e só existe no Windows.

### Por que `clip-set` não escreve direto

No Linux, quem escreve a seleção tem de **continuar vivo** para
responder `SelectionTransfer` quando alguém colar. Um CLI que escreve e
sai deixaria seleção sem dono, e o texto sumiria no primeiro Ctrl+V.
Então o CLI entrega ao daemon local (POST em 127.0.0.1) e o daemon
fica.

### Laço de eco

A mandaria para B, B veria o clipboard mudar e mandaria de volta, para
sempre. `registrar()` guarda o último texto visto **ou escrito**, nas
duas direções; mudança que bate com ele não é reenviada.

### Windows e macOS: amostragem

Os dois têm API de notificação (`AddClipboardFormatListener`,
`changeCount` do `NSPasteboard`) que exigiria laço de mensagens ou objc
dentro de um daemon que não tem nem um nem outro. Amostragem de 2 s
resolve: não dá para sentir ao trocar de máquina.

Detalhe que custaria tempo: o `Get-Clipboard` do PowerShell acrescenta
CRLF no fim. Sem aparar, cada leitura pareceria texto novo e a máquina
empurraria o mesmo conteúdo para sempre.

### Conferido

As quatro direções, com marca de tempo em cada uma:

| De → Para | Resultado |
|---|---|
| Ubuntu → Mac | `ubuntu-171000` |
| Ubuntu → Windows | `ubuntu-171000` |
| Mac → Ubuntu | `mac-171127` |
| Windows → Ubuntu | `windows-171146` |

O clipboard do Windows tem de ser lido **dentro da sessão interativa**:
cada sessão tem sua própria window station, e um `Get-Clipboard` pelo
SSH lê o da sessão 0, que é outro. Mesma fronteira que já apareceu no
cursor e no host de tela.

### Limites conhecidos

- **Só texto.** Imagem e arquivo não viajam. O KDE Connect fazia; se
  fizer falta, é trabalho novo.
- 256 KB de teto. Clipboard de desenvolvedor às vezes carrega um
  arquivo inteiro colado sem querer.
- O Mac precisou da chave dele autorizada no Ubuntu (`Host ubuntu` no
  `~/.ssh/config` dele já existia; faltava o lado de cá aceitar).
