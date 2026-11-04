# logi — porta de entrada para agentes

Infra pessoal de **três** computadores na mesma LAN: Ubuntu
(principal), Windows ao lado, MacBook Air. Começou como "um Logitech
Flow que funcione no Linux" e virou mais: troca de máquina, tela
remota própria, e o controle de tudo num painel só.

| Diretório | Conteúdo | Quando consultar |
|---|---|---|
| [`.claude/rules/`](.claude/rules/) | Regras invioláveis — armadilhas que já custaram tempo | Antes de mexer em qualquer máquina |
| [`.claude/skills/`](.claude/skills/) | Procedimentos — receitas que funcionaram | Quando a tarefa for "como faço X de novo" |
| [`docs/`](docs/) | Conhecimento de domínio com [[wikilinks]] | Para entender como as peças se encaixam |
| [`specs/`](specs/) | Especificações, de `000-overview` a `010-remote-desktop` | Para entender *por que* o sistema é assim |
| [`README.md`](README.md) | Runbook histórico — o que testei e falhou | Antes de reintroduzir algo que já saiu |

## Regras (resumo executivo)

1. [Cada máquina tem a própria config, e nada sincroniza](.claude/rules/config-por-maquina.md)
2. [Reiniciar agente: o nome do processo não é o nome do binário](.claude/rules/reiniciar-agente.md)
3. [PowerShell remoto: sempre `-EncodedCommand` a partir de arquivo](.claude/rules/powershell-remoto.md)
4. [`E_ACCESSDENIED` no DXGI: quatro causas, e a mais comum é o protetor de tela](.claude/rules/tela-bloqueada-windows.md)
5. [Windows sem mouse não tem cursor, e desenhar no stream não ajuda quem olha o monitor](.claude/rules/cursor-sem-mouse.md)
6. [Mandar não é ter chegado; número que ninguém imprime ninguém confere](.claude/rules/verificar-o-que-chegou.md)

## Skills (procedimentos)

- [Deploy nas três máquinas](.claude/skills/deploy.md)
- [Mudar config nas três máquinas](.claude/skills/mudar-config-nas-tres.md)
- [Dar acesso aos dispositivos de input (KVM)](.claude/skills/dar-acesso-a-input.md)
- [Testar o host de tela com a tela apagada](.claude/skills/testar-tela-apagada.md)

## Docs

- [[docs/index]] — índice
- [[docs/maquinas]] — as três máquinas, caminhos e nomes que divergem
- [[docs/caminhos-de-tela]] — quem captura, codifica e decodifica em cada par

## Os componentes

| Dir | O quê |
|---|---|
| [`logid/`](logid/) | Control plane em Rust: API, painel, troca de máquina, probes |
| [`logistream/`](logistream/) | Tela remota e KVM: captura, NVENC/NVDEC, QUIC, input |
| [`hidpp/`](hidpp/) | HID++ 2.0 sobre HID cru — troca de canal Easy-Switch |
| [`gui/`](gui/) | Casca Electron: janela e bandeja. **Sem lógica** |
| [`agent/`](agent/) | Lado Windows: CLI por SSH |

## Decisões que não se refazem sem justificar

**O Deskflow e o Solaar saíram.** A troca de máquina é física (HID++
0x1814) e a detecção de borda é nossa. O Solaar competia pelo mesmo
receptor.

**Nunca escreva IP em config.** O IP do Windows já mudou uma vez e
quebrou Deskflow, SSH e o painel de uma vez. Use mDNS.

**O RDP saiu** (04/11): os quatro caminhos de tela são nossos. O KDE
Connect sai quando o clipboard nativo entrar — e **não antes**, porque
hoje é ele que dá clipboard entre as máquinas.

**Um dongle só.** Dois receptores Logitech perto não funcionam: a cola
segue o dongle, não a máquina. Ver [`specs/005`](specs/005-input-switch.md).

## Regras de trabalho neste repo

1. **Verifique, não presuma.** Metade dos achados aqui contradiz a
   documentação oficial — e metade contradiz o que eu mesmo escrevi no
   commit anterior. O `InputCapture` do GNOME existe e **não** serve
   para ativar captura sob demanda; a "tela bloqueada" que eu
   diagnostiquei era tela apagada; o dongle "defeituoso" não está
   provado. Teste antes de afirmar, e corrija o que já afirmou.
2. **Meça antes de otimizar, e imprima o que mediu.** Dois bugs de
   semanas sobreviveram por falta de uma linha de log.
3. **Registre as falhas, não só os sucessos.** Vale mais que o que
   funciona, porque evita repetir a tarde perdida.
4. **UI em pt-BR, código e commits em inglês.**
5. **Nada de telemetria, nada de nuvem.** Tudo na LAN. O trust model
   está em [`specs/008-security.md`](specs/008-security.md).
