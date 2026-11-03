# logi — porta de entrada para agentes

Infra pessoal de dois computadores na mesma LAN: um Ubuntu (principal)
e um Windows ao lado. O objetivo é um **Logitech Flow que funcione no
Linux**, e mais do que isso.

| Arquivo / dir | Conteúdo |
|---|---|
| [`README.md`](README.md) | **Runbook operacional** — o que está instalado, o que testei e falhou, e o porquê de cada escolha. Leia antes de mexer em qualquer coisa. |
| [`specs/`](specs/) | **Especificações** do sistema que queremos construir. Começa em [`specs/000-overview.md`](specs/000-overview.md). |
| [`monitor/`](monitor/) | Painel de status já funcionando (Python stdlib + casca Electron). |
| `install.sh`, `status.sh`, `windows-*.ps1` | Scripts idempotentes de setup e diagnóstico. |

## As duas máquinas

| | Ubuntu | Windows |
|---|---|---|
| nome | `ubuntu-host` | `windows-host` / `windows-host.local` |
| SO | Ubuntu 26.04, GNOME 50.1, **Wayland** | Windows 10 Pro |
| usuário | `user` | `user` |
| acesso | — | SSH por chave (`ssh windows-host`) |

**Nunca escreva IP em config.** O IP do Windows já mudou uma vez e
quebrou Deskflow, SSH e o painel de uma vez. Use `windows-host.local`
(mDNS via avahi + `libnss-mdns`). Ver a seção *"O IP muda"* no README.

## Regras de trabalho neste repo

1. **Verifique, não presuma.** Metade dos achados aqui contradiz a
   documentação oficial dos projetos (o `InputCapture` que o GNOME tem
   e o Deskflow diz não existir; a lista com vírgula do `remoteHost`
   que o tooltip promete e o core rejeita). Teste antes de afirmar.
2. **Registre as falhas, não só os sucessos.** O README tem seções
   inteiras sobre o que *não* funciona. Isso vale mais que o que
   funciona, porque evita repetir a tarde perdida.
3. **Integrar > reimplementar.** Ver o critério em
   [`specs/000-overview.md`](specs/000-overview.md#o-que-construímos-e-o-que-integramos).
4. **UI em pt-BR, código e commits em inglês.** Mesma convenção do
   outro repo.
5. **Nada de telemetria, nada de nuvem.** Tudo na LAN. O trust model
   está em [`specs/008-security.md`](specs/008-security.md).
