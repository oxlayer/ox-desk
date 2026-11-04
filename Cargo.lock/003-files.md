# 003 — File browser

## Decisão: SFTP, não protocolo novo

O OpenSSH já está nas duas máquinas e o SFTP vem junto. Não há motivo
para inventar transporte: o que falta é **UI**, não protocolo.

O KDE Connect tem plugin `sftp` (está em `incoming-capabilities` do
`windows-host`), mas depende do daemon dele e monta via KIO — fora do
nosso controle. SSH direto é mais simples e já autenticado.

## Duas camadas

**Navegação (`/api/files/{id}?path=`)** — lista diretório via
`sftp` em modo batch. Só leitura, rápido, sem montar nada:

```bash
echo "ls -la /caminho" | sftp -b - windows-host
```

**Montagem (opcional)** — `sshfs` (já instalado aqui) para quando você
quer abrir no Nautilus ou num editor:

```bash
sshfs windows-host:/C:/Users/user ~/mnt/windows-host -o reconnect,idmap=user
```

Atenção: `sshfs` morre feio quando o host cai, e o host **já caiu uma
vez hoje** por troca de IP. `-o reconnect` ajuda; mesmo assim, montagem
é conveniência opcional, nunca o caminho principal da UI.

## Caminhos no Windows

O SFTP do Windows expõe `/C:/Users/user/...`. A UI deve mostrar
`C:\Users\user` e traduzir internamente — o usuário não deve ver a
costura.

## Transferência

Arquivo pequeno: `scp`/`sftp` direto.
Arquivo que você quer "mandar para a outra máquina" como gesto:
o **share do KDE Connect** já funciona e cai em
`~/Downloads` (`receive-directory` no dconf). Não reimplementar.

## Não fazer

- Não indexar nem cachear árvore de diretórios. Latência de LAN é baixa;
  cache inválido é pior que listar de novo.
- Não escrever por padrão. Escrita só com ação explícita do usuário.

---

## Implementado (03/11)

[`logid/src/files.rs`](../logid/src/files.rs) + painel "Arquivos" no
dashboard. **Só listagem** nesta versão: não baixa, não envia, não apaga.

O daemon é superfície de privilégio ([`008`](008-security.md)) e já pode
executar SSH, então listar não concede nada novo — mas escrever concede,
e escrever por acidente num painel que eu mesmo sirvo é o tipo de erro
que não tem desfazer.

### Rota

`GET /api/files?host=<nome>&path=<caminho>`

Sem `host`, usa o host `local` da config — o caso comum de abrir o painel
e só olhar. Sem `path`, cada host começa no próprio home (`pwd` do SFTP
no remoto, `$HOME`/`%USERPROFILE%` no local). O `path` vem
percent-encoded porque carrega `/`, `\`, `:` e espaço, e nenhum deles
pode ser confundido com separador de query.

A rota ficou `?host=` em vez do `/api/files/{id}` que esta spec propunha:
os hosts já são identificados por **nome** em todo o resto da API
(`/api/focus` recebe `host`), e introduzir um `id` paralelo seria um
segundo jeito de dizer a mesma coisa.

### Um lote de SFTP, não dois

A primeira versão chamava `pwd` e depois `cd`+`ls -la`, pagando dois
handshakes SSH: **809 ms** medidos para listar uma pasta. Com `pwd` e
`ls` no mesmo lote: **350 ms**.

De brinde, o `pwd` no mesmo lote dá o caminho canônico confirmado pelo
servidor, e é dele que saem o `display` e o `parent` — a UI navega pelo
que o servidor diz, não pelo que eu supus.

### O parser é ancorado no mês, não em contagem de campos

Escrevi "nome = tudo depois do 8º campo". São **nove** (modo, links,
dono, grupo, tamanho, mês, dia, hora, nome), e o teste pegou: `.` e `..`
passavam pelo filtro porque o nome lido era outro pedaço da linha, e o
"tamanho" lido era o nome do grupo.

Corrigir o índice para 9 não bastaria — há servidor SFTP que omite o
grupo, e aí são oito. Procurar o campo do **mês** acha a junção nos dois
casos, porque dali para a direita o formato é fixo (`<mês> <dia>
<hora-ou-ano> <nome>`) e o tamanho é o campo imediatamente anterior.
Travado por `parseia_mesmo_sem_o_campo_de_grupo` e
`ano_em_vez_de_hora_para_arquivo_antigo`.

O nome é recortado por **deslocamento de byte** na linha original, não
por `join` dos campos restantes: `Program Files` tem espaço, e juntar
colapsaria espaços repetidos.

### Caminhos não são comparáveis entre as duas origens

O SFTP expõe o disco do Windows como `/C:/Users/user`; o `std::fs` na
própria máquina diz `C:\Users\user`. São dois nomes para o mesmo lugar
e nenhum está errado. Em vez de normalizar (e errar), cada listagem
declara sua `source`, devolve `path` **opaco** para a UI navegar, e um
`display` separado para mostrar — `C:\Users\user` na tela, `/C:/...` no
clique.

### Data: cada origem diz o que de fato sabe

Local entrega época unix (`mtime`), SFTP entrega o texto do `ls`
(`mtime_text`) porque o protocolo não dá época nessa listagem. A UI
prefere a época e formata no fuso local; cai no texto quando é o que
existe. Inventar precisão que a origem não tem seria pior que mostrar
`Nov 3 22:19`.

### Verificado

Teto de 2000 entradas por listagem (`truncated` avisa). 17 testes
unitários. Fim-a-fim pela própria UI, num Electron headless:

- `ubuntu-host` → `/home/user`, 84 itens, `local`
- clique em `windows-host` → `C:\Users\user`, 105 itens, `sftp`
- entra em `.agents` → `C:\Users\user\.agents`
- clique em "voltar" → volta para `C:\Users\user`, 105 itens

### Falta

- **Windows → Ubuntu não lista**, e diz isso na tela em vez de ficar
  girando: o `openssh-server` não está instalado no Ubuntu. Mesmo
  bloqueio da simetria da borda em [`001`](001-control-plane.md).
- Transferência e `sshfs`: desenhados acima, não implementados.
- Mac: entra junto com o host novo.
