# 016 — A tela inicial e a sessão

> Estado: proposta. Nada implementado.

## O problema

O painel responde "o que está de pé", mas não parece um produto: a ação
principal, abrir a tela de uma máquina, disputa espaço com portas, RAM e
opções. E depois de conectar não há nada além do vídeo: para trocar um
arquivo é preciso sair da sessão.

Parsec e AnyDesk são referência de **organização**, e não de marca: a
identidade visual continua nossa.

## O que muda

### 1. Tela inicial

- Um cartão por máquina: nome, sistema, estado e **Conectar** como botão
  principal.
- Portas, RAM, CPU e serviços ficam num detalhe que se abre.
- Só aparece a opção que o cliente sabe cumprir.
- Busca só a partir de cinco máquinas. Com três, ela é ruído.

### 2. A sessão é uma aba

Conectar abre uma aba na janela do painel, e não outra janela. Várias
máquinas ficam abertas lado a lado, e a tela inicial é a primeira aba.

A barra fica no topo da aba, e não pode roubar clique da máquina remota.

| Botão | Faz |
|---|---|
| Arquivos | abre o gerenciador nesta máquina |
| Tela | escolhe o monitor, quando houver mais de um |
| Qualidade | muda o teto de `--mbps` |
| Teclado | manda Ctrl+Alt+Del e atalhos do sistema |
| Sair | encerra a sessão |

### 3. Gerenciador de arquivos

Dois painéis: esta máquina à esquerda, a remota à direita, com caminho,
voltar, subir, recarregar, e as colunas nome, tamanho e data. Enviar e
receber mostram progresso e erro por arquivo.

## Como

**O vídeo é decodificado na página.** O `logistream-view` é outro processo
com janela nativa, e isso não se embute numa aba. Uma ponte local fala QUIC
com o host e entrega H.264 à página por WebSocket em `127.0.0.1`, com um
token e checagem de `Origin`. A página decodifica com WebCodecs e desenha
num `<canvas>`; teclado e mouse fazem o caminho inverso.

**A decodificação é por software.** O fluxo não declara que não reordena, e o
decodificador de hardware segura cerca de 4 quadros: 172 ms contra 1,9 ms
(p50, 1080p, no Mac). Para voltar ao hardware, o host precisa mandar
`max_num_reorder_frames=0` no VUI do H.264.

Com isso aba, barra e arquivos são a mesma página, e a barra é HTML. O
`logistream-view` fica como alternativa até a latência da aba estar medida:
ela só substitui o nativo se não for pior.

**O gerenciador é uma aba do mesmo jeito.** Ver a [spec 003](003-files.md).

**A transferência vai por SFTP**, como a listagem. A rota nova é
`POST /api/files/transfer`, com o progresso lido pelo painel.

O `logid` continua escutando só em `127.0.0.1` (spec 008). Quem transfere é
ele, com a chave que já tem.

## Regras que isto herda

- **A prova de tela é olhar a imagem.** Contar quadro não prova nada
  ([[../docs/caminhos-de-tela]]).
- **Clique na barra não vai para a máquina remota.** O teste confere que
  nenhum `Evento::Mouse` saiu.
- **A aba se mede contra o `logistream-view`:** o atraso da captura à tela.
- **Transferência se prova pelo hash dos dois lados**, e não pelo fim da
  barra de progresso ([[../.claude/rules/verificar-o-que-chegou]]).
- **O caminho do Windows é o do SFTP** (`/C:/Users/...`), que já tem
  conversão em `files.rs`.

## Ordem

1. Tela inicial, sem mexer em API. É só `static/index.html`.
2. Ponte e aba da sessão, com a latência medida contra o nativo.
3. Barra da sessão, com os atalhos.
4. Gerenciador de arquivos e a rota de transferência.
5. Qualidade e troca de monitor, que dependem de mudar o host em sessão.

## Fora daqui

- Pareamento por chave do canal de tela (spec 012).
- Áudio remoto, chat e gravação de sessão.
