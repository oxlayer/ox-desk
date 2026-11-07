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

- Trilho lateral com Máquinas, Arquivos, Configurar e Ajuda.
- Um cartão por máquina: nome, sistema, estado e **Conectar** como botão
  principal.
- Portas, RAM, CPU e serviços ficam num detalhe que se abre.
- Busca só a partir de cinco máquinas. Com três, ela é ruído.
- Tema escuro como padrão, mantendo o claro.

### 2. Barra da sessão

Uma barra fina que aparece quando o ponteiro chega à borda de cima do
visualizador, e some depois. Ela não pode roubar clique da máquina remota.

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

**A barra é desenhada no visualizador.** Ele é uma janela `winit` com
`softbuffer`, sem toolkit, e a barra são ícones em bitmap compostos sobre
o quadro. Puxar `egui` para isso custaria mais que a barra inteira.

**O gerenciador é uma página do `logid`** (`/arquivos`), aberta pela barra.
HTML resolve tabela, rolagem e arrastar sem reescrever isso em pixel, e o
`gui/` continua sem lógica. Ver a [spec 003](003-files.md).

**A transferência vai por SFTP**, como a listagem. A rota nova é
`POST /api/files/transfer`, com o progresso lido pelo painel.

O `logid` continua escutando só em `127.0.0.1` (spec 008). Quem transfere é
ele, com a chave que já tem.

## Regras que isto herda

- **A prova de tela é olhar a imagem.** Contar quadro não prova nada
  ([[../docs/caminhos-de-tela]]).
- **Clique na barra não vai para a máquina remota.** O teste confere que
  nenhum `Evento::Mouse` saiu.
- **Transferência se prova pelo hash dos dois lados**, e não pelo fim da
  barra de progresso ([[../.claude/rules/verificar-o-que-chegou]]).
- **O caminho do Windows é o do SFTP** (`/C:/Users/...`), que já tem
  conversão em `files.rs`.

## Ordem

1. Tela inicial, sem mexer em API. É só `static/index.html`.
2. Gerenciador de arquivos e a rota de transferência.
3. Barra da sessão, abrindo o gerenciador e os atalhos.
4. Qualidade e troca de monitor, que dependem de mudar o host em sessão.

## Fora daqui

- Pareamento por chave do canal de tela (spec 012).
- Áudio remoto, chat e gravação de sessão.
