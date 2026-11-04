# 008 — Segurança e trust model

Tudo na LAN. Nenhuma conta, nenhuma nuvem, nenhuma telemetria.
Auditado nos binários que já usamos (ver README): Deskflow, Solaar e
GSConnect não têm endpoint de telemetria; o único capaz de falar com a
internet é o update check opt-in do Deskflow, que vem
`enableUpdateCheck=false`.

## Por que só localhost

O `logid` escuta em `127.0.0.1`, não em `0.0.0.0`. Ele **lê portas,
executa SSH e dispara troca de canal HID++** — é superfície de
privilégio, não um site. Quem alcançar a API alcança as duas máquinas.

Se um dia quiser abrir para o celular, não bind em `0.0.0.0`: exponha
por SSH tunnel ou por um reverse proxy com auth. A diferença é
intencional.

## Confiança entre as máquinas

Três pares de credencial, cada um da ferramenta dona. **Não
centralizar** — reinventar key management é como se perdem chaves.

| Canal | Credencial | Verificação |
|---|---|---|
| SSH | ed25519 **com passphrase**, no `gcr-ssh-agent` | fingerprint em `known_hosts`, conferida ao migrar IP→nome |
| Deskflow | TLS, cert em `~/.config/Deskflow/tls/` | fingerprint aprovado no primeiro contato (`trusted-servers`) |
| KDE Connect | TLS mútuo, cert por device | *verification key* `4BCF0AF8` |

Os três usam **trust on first use com confirmação humana**. Isso não é
atrito a eliminar: é a única defesa contra alguém na mesma LAN se passar
pela outra máquina. Um "pareamento de um clique" que aceita fingerprint
sozinho remove a proteção inteira.

**Regra:** o `logid` pode *mostrar* os fingerprints lado a lado para
facilitar a comparação. Nunca pode aceitar por você.

## Riscos ligados hoje, conscientemente

| Risco | Estado | Mitigação |
|---|---|---|
| RDP do GNOME na 3389, senha, toda a LAN | **ligado** | `grdctl rdp disable`; o painel deve mostrar isso como risco |
| Clipboard espelhando senha do Bitwarden | mitigado | `send-content=false` — unidirecional ([`004`](004-clipboard.md)) |
| Chave SSH com acesso admin no Windows | aceito | passphrase + agente de sessão |
| `hidraw` acessível ao usuário | necessário | `uaccess` por udev, só usuário logado localmente |
| Input trafegando na LAN (Deskflow) | aceito | TLS 1.3, fingerprint conferido |

## O que nunca gravar

- Passphrase, senha ou chave privada em `config.toml` ou no estado.
- Conteúdo de clipboard em log.
- Fingerprint aceito automaticamente, sem registro de quando e por quem.

## Modo de falha que é problema de segurança

Perder teclado e mouse nas duas máquinas (ver
[`005`](005-input-switch.md#rollback-não-é-opcional)) te deixa sem como
intervir numa máquina destravada. Rollback automático não é conveniência:
é controle de acesso.

---

## O Defender apagou o daemon, e estava sendo razoável (04/11)

`Trojan:Win32/Bearfoos.A!ml` — **quarentena e remoção** do
`logid.exe`, mais a exclusão da tarefa agendada e das chaves
`TaskCache` correspondentes. No meio de uma sessão de testes, o que
explicou de quebra o "trava e não vai nem com reza": não havia daemon.

### Por que ele implicou

O sufixo **`!ml`** é detecção por modelo, não assinatura — nenhum
malware conhecido casou. O que casou foi o **perfil**, e é justo:

| o que o daemon faz | como um modelo lê isso |
|---|---|
| executável novo, sem assinatura, sem reputação | binário recém-plantado |
| cria tarefa agendada que roda no login | persistência |
| executa `ssh`, `taskkill`, `tasklist`, `sc` | execução remota de comandos |
| lê dispositivo HID em modo bruto | captura de entrada |
| sobe servidor HTTP local | canal de controle |

Cada item foi pedido. Juntos, desenham a silhueta de um RAT. A
detecção listou o **arquivo e a tarefa juntos** — foi o conjunto.

### A saída escolhida: assinar, não excluir

O usuário preferiu assinar a abrir exceção no antivírus. Exclusão
resolveria em um comando, mas desliga inspeção de um caminho — e a
inspeção é exatamente o que protege contra alguém escrevendo ali.

**Assinar no Windows não funcionou**: a proteção em tempo real removeu o
`.exe` no instante da escrita, duas vezes, antes de haver chance de
assinar. É uma corrida que não se ganha do lado de lá.

Então o binário passa a chegar **já assinado**, por
[`scripts/sign-windows.sh`](../scripts/sign-windows.sh):

- par gerado **aqui** com openssl (`~/.config/logi/signing`, `600`, fora
  do repo). A chave privada nunca sai desta máquina — melhor que exportar
  a chave privada de um certificado criado no Windows.
- `osslsigncode` (instalado por linuxbrew, sem sudo) assina com SHA-256 e
  carimbo de tempo da DigiCert, então a assinatura sobrevive ao
  vencimento do certificado.
- só o certificado **público** vai para o Windows, importado em
  `LocalMachine\Root` e `LocalMachine\TrustedPublisher`. Vale naquela
  máquina e em mais nenhuma.

Resultado conferido pelo próprio Windows:

```
assinatura: [Valid] Assinatura verificada.
assinante:  O=infra pessoal, CN=logi (assinatura local)
carimbo:    CN=DigiCert SHA256 RSA4096 Timestamp Responder 2026 1
```

O arquivo sobreviveu à escrita, a tarefa foi recriada e o daemon subiu.

> **Todo build para Windows passa pelo script.** Mandar um `.exe` cru
> para lá não é só arriscado — já foi apagado duas vezes, e a segunda
> levou a tarefa agendada junto.

### Se voltar a acontecer

Assinatura própria não garante nada contra heurística de comportamento:
o modelo pesa o que o programa faz, e o que ele faz não mudou. Se
reaparecer, as saídas são, em ordem de menor dano:

1. exclusão do **caminho exato do arquivo** (não da pasta) mais o
   processo — escopo de um arquivo;
2. envio a Microsoft como falso positivo (demora dias, mas conserta para
   todo mundo);
3. exclusão da pasta — maior escopo, último recurso.

## Exclusão do Defender, e por que ela foi necessária (04/11)

O Defender agiu três vezes sobre os nossos binários — o histórico dele
lista `logid.exe` e `logid.new.exe` como **`Trojan:Win32/Bearfoos.A!ml`**.
O sufixo `!ml` é palpite de modelo, não assinatura de malware conhecido;
a silhueta (executável novo, sem reputação, que cria tarefa agendada,
executa comando remoto e lê dispositivo de entrada em modo bruto) é
parecida de verdade.

Assinar resolveu para o `logid.exe`. **Não resolveu para o
`logid-agent.exe`**: os mesmos bytes assinados eram aceitos com outro
nome e revertidos em silêncio com o nome certo. O Windows ficou uma hora
rodando a versão anterior enquanto o deploy dizia `ok`.

O usuário autorizou a exclusão de `%LOCALAPPDATA%\logid`. O custo é
real e fica escrito aqui: **o Defender deixou de examinar aquela pasta**,
então o que cair lá não é verificado. Em troca, a entrega deixou de
depender de um heurístico que muda de opinião entre versões de assinatura.

### A lição que vale mais que a exclusão

O passo do deploy dizia **"ok" sem conferir o resultado**. `Move-Item`
não reclamava, o antivírus revertia depois, e o relatório mentia. Agora
o tamanho esperado viaja no comando e o passo compara depois de gravar.

E a verificação foi testada com um tamanho errado de propósito, até
falhar:

```
ERRO: gravou mas o arquivo tem 458792 bytes, esperado 999999
      -- antivirus revertendo? confira a exclusao de C:\...\logid
```

Verificação que nunca falhou não é verificação conhecida. Vale para as
outras etapas do deploy que hoje só olham a assinatura.
