# Configurar uma máquina (wizard de setup)

Abra `http://127.0.0.1:8787/setup` **na máquina que você quer
configurar**. Pela bandeja: "Configurar esta máquina". A casca Electron
abre essa página sozinha, em tela cheia, quando falta algo essencial.

```bash
xdg-open http://127.0.0.1:8787/setup     # Ubuntu
ssh windows-host 'start http://127.0.0.1:8787/setup'   # não serve: ver abaixo
```

> ⚠️ **Não dá para configurar uma máquina a partir de outra.** O wizard
> mede a sessão gráfica de quem o serve. Por SSH o Windows cai na sessão
> 0 (área de transferência sempre vazia) e o macOS não tem concessão de
> TCC nenhuma. Ver [[../rules/graphical-session]].

## Ler a tela

Quatro estados, e o terceiro é o que importa:

| Marca | Significa |
|---|---|
| ✓ verde | medido e de pé |
| ! vermelho | medido e falta |
| ? âmbar | **não consegui medir** — não é "ok" |
| – apagado | não se aplica a esta máquina |

O passo destacado com o anel azul é onde agir **agora**. Passos
apagados com "espera por: X" dependem de outro que ainda falta — não
adianta mexer neles antes.

A barra conta só os **essenciais**. Opcionais pendentes aparecem no
canto direito ("2 opcional(is) pendente(s)") e não impedem o "pronto".

## Pela linha de comando

```bash
curl -s http://127.0.0.1:8787/api/setup | python3 -m json.tool
curl -s -X POST http://127.0.0.1:8787/api/setup/arrumar \
     -H 'Content-Type: application/json' -d '{"id":"daemon"}'
```

`/api/setup` leva alguns segundos: o passo `pares` abre SSH para cada
máquina. É por isso que a página **não** se atualiza sozinha.

## Quando o botão diz que não resolveu

Ele está dizendo o que **mediu depois** de agir, não que o comando
falhou. Dois casos normais:

- **TCC do macOS**: o reparo abre o painel do sistema e volta "marque a
  caixa na janela que abriu". É o fluxo esperado — marque e clique
  "Medir de novo".
- **`sudo`**: o wizard nunca roda sudo. Copie a linha, cole num
  terminal, volte e meça.

## Ordem que importa no Mac

`Assinar` vem **antes** de pedir as permissões, e o wizard impõe isso
por dependência. Conceder Gravação de Tela a um binário ad-hoc
desperdiça a concessão: ela morre no próximo `deploy.sh`, porque o TCC
amarra no cdhash, que muda a cada compilação. Já aconteceu duas vezes.

## Tailscale no Mac

O passo `tailscale` é opcional e mede pelo `tailscale` do brew. Com o
app instalado ele aparece como falta, mesmo conectado: apague o app pelo
Finder e **negue** qualquer pedido de VPN. Ver
[[../../specs/012-outside-the-lan]].

```bash
brew install tailscale
sudo bash logid/setup/instalar-tailscale-mac.sh   # no clone do repo
```

O botão do wizard faz o mesmo sem clone: abre o Terminal com o script.

## Windows sem monitor

O passo `tela-virtual` conta os monitores presentes. Com zero, o
visualizador conecta e nenhum quadro chega. O botão instala o driver da
`logid/setup/instalar-tela-virtual.ps1`, com versão e hash fixados. Por
SSH, como administrador, o script roda direto, sem pedido na tela.

## Computador novo

A ordem por sistema está em [[../../docs/new-computer]].

## O que ele não faz

Não instala binário (use `install.sh` ou `scripts/deploy.sh`), não roda
sudo, e não fica medindo em laço.

## Conferir que a página ainda funciona

```bash
cd gui && env -u ELECTRON_RUN_AS_NODE \
  ./node_modules/electron/dist/electron check-setup.js
```

Sai `ERROS []` quando passa. O teste compara a tela com o que a API
mediu — contador, número de linhas, destaque do passo ativo, nenhuma
linha sem detalhe, nenhuma rolagem horizontal em 420 px. Ele já pegou um
acordeão que mantinha dois passos abertos enquanto o estado dizia um.

## Veja também

- [[../../specs/011-onboarding]] — o desenho e as três regras
- [[deploy]] — o que leva binário para as três máquinas
- [[verify-the-mac]] — quando a tela do Mac não abre
- [[grant-input-access]] — a regra de udev em detalhe
