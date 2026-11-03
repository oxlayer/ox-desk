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
