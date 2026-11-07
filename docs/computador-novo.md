# Computador novo

O objetivo é não depender de memória: instalar o `logid`, abrir o wizard
em `http://127.0.0.1:8787/setup` e apertar os botões na ordem que ele
mostra. Os instaladores vão dentro do binário, então não precisa do repo.

## Mac

1. Instale o [Homebrew](https://brew.sh) e o `logid`.
2. No wizard, aperte os passos em ordem. As permissões vêm depois de
   `Assinar`.
3. No passo **Tailscale**, o Terminal abre: digite a senha e abra o link
   de login. **Não instale o app** ([`specs/012`](../specs/012-fora-da-lan.md)).

## Windows

1. Instale o `logid` e o Tailscale, e faça o login no Tailscale.
2. No wizard, aperte os passos em ordem.
3. No passo **Tela sem monitor ligado**, aceite o pedido de administrador.

## Ubuntu

1. `./install.sh` e `sudo bash scripts/instalar-tailscale.sh`.
2. No wizard, aperte os passos em ordem. Sem monitor ligado o host usa
   uma tela virtual sozinho, e não há passo para isso.

## O que continua manual

- O login do Tailscale: é o consentimento de quem é dono da conta.
- As permissões do macOS: a Apple não deixa dar de fora.
- O token do OTA, que se cria uma vez e se distribui com
  `scripts/distribuir-token-ota.sh`
  ([skill](../.claude/skills/ligar-o-ota.md)).
