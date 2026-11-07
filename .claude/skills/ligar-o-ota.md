# Ligar e operar a atualização automática (OTA)

O que faz a máquina baixar o build publicado e se trocar sozinha, sem
ninguém rodar o deploy. Desenho e justificativas:
[`specs/015`](../../specs/015-ota.md).

## 1. O token (uma vez, e é a única parte manual)

O repo é privado, então sem token a API responde `200` com
`{"message":"Not Found"}` e nada funciona.

Crie um **fine-grained PAT** em
<https://github.com/settings/personal-access-tokens/new>:

| Campo | Valor |
|---|---|
| Resource owner | `oxlayer` |
| Repository access | Only select repositories → `ox-desk` |
| Permissions → Repository | **Contents: Read-only** |
| Expiration | o que você aceitar renovar |

Nada além de `Contents: Read-only`. **Não** use o token do `gh`: ele
tem `repo` e `workflow`, ou seja, escrita em tudo — e este arquivo vai
para um laptop que sai de casa.

```bash
install -m600 /dev/null ~/.config/logi/ota-token
printf '%s' 'github_pat_...' > ~/.config/logi/ota-token
```

O `deploy.sh` distribui para o Windows e o Mac a partir daqui (e dá
`chmod 600` do lado de lá). O wizard **recusa** um token legível por
outros.

## 2. Ligar o relógio em cada máquina

No wizard (`/setup` no painel, ou o app abre nele sozinho quando falta
algo essencial), o passo **Atualização automática (OTA)** tem botão. Ele
instala o agendador do sistema — timer do systemd, LaunchAgent,
tarefa agendada — e faz uma primeira checagem em modo `--conferir`.

## 3. Publicar uma versão para as máquinas consumirem

```bash
git tag v0.3.0 && git push origin v0.3.0
```

Sem tag não há Release, e sem Release o OTA não tem o que baixar — ele
diz `igual` para sempre. Receita e custo em
[`specs/013`](../../specs/013-ci-cd.md#publicar-uma-versão).

## Perguntar à mão

```bash
logi-ota --conferir   # só olha, não troca nada
logi-ota --estado     # o JSON da última checagem
logi-ota              # confere e aplica, se houver novidade
logi-ota --forcar     # reinstala o publicado mesmo sendo o mesmo build
```

No **Windows** nenhum deles troca binário: a chave do Authenticode não
sai do Ubuntu, então lá o OTA só confere e relata. É esperado, e o
painel mostra "há o build #N publicado" como pendência permanente até
alguém rodar o deploy.

## Quando der errado

| Sintoma | O que é |
|---|---|
| `o GitHub respondeu: Not Found` | falta o token, ou ele não dá `Contents: read` neste repo |
| `o GitHub respondeu: Bad credentials` | token inválido ou expirado |
| `nenhum pacote comecando com logi-mac-...` | a Release saiu sem aquele pacote (job de Mac sem runner?) |
| `o pacote nao confere` | download corrompido; a próxima rodada tenta de novo |
| `revertido` | o build novo não respondeu ao `/healthz`; o anterior voltou. A cópia fica em `~/.local/state/logi-ota/anterior/` |
| `o relógio não está acordando` | a última checagem foi há mais de 6 h — o agendador caiu |

Depois de um boot, o Mac pode levar **até uma hora** para olhar: o
LaunchAgent é `RunAtLoad false` de propósito, porque instalá-lo não pode
aplicar um update no meio da requisição que o instalou.

## Veja também

- [[deploy]] — o caminho de dentro de casa, que assina e troca tudo
- [[configurar-maquina]] — o wizard onde o passo vive
