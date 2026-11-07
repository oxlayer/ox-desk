# O que depende da sessão gráfica não nasce por SSH

Três sistemas, três mecanismos, **a mesma regra**: captura de tela e
injeção de input só funcionam em processo lançado de dentro da sessão
do usuário. SSH não é isso em nenhum deles.

| Sistema | Onde o SSH cai | Como lançar de verdade |
|---|---|---|
| Windows | sessão 0, sem desktop | tarefa agendada com `/IT` |
| macOS | sessão sem concessão TCC | LaunchAgent (`gui/$(id -u)/...`) |
| Linux | funciona, mas o processo morre com a conexão | `setsid nohup` |

## macOS é o pior dos três, porque não falha

No Windows o erro aparece: `E_ACCESSDENIED`, `SendInput` recusado. No
macOS a captura **trava**. Sem concessão de Gravação de Tela, o
AVFoundation simplesmente nunca retorna.

Medido para não restar dúvida de que era nosso código:

```
ffmpeg -f avfoundation -framerate 60 -capture_cursor 1 -i 1 -frames:v 5 -f null -
```

pendura para sempre quando chamado por SSH. O mesmo comando na sessão
gráfica roda. Nosso host parava antes do primeiro `println!`, com o
processo em estado `S` e sem socket aberto — sintoma de bloqueio, não
de erro.

## A concessão é manual, e isso não tem volta

Gravação de Tela se concede em **Ajustes → Privacidade e Segurança →
Gravação de Tela**, e não dá para fazer de fora: é exatamente a
proteção que a Apple construiu. Para um LaunchAgent sem interface o
sistema pode nem mostrar o diálogo.

Depois de concedida, tudo destrava de uma vez:

```
avfoundation: tela 1, 3360x2100, UYVY422
h264_videotoolbox pronto, 28 Mbps alvo
host na porta 9777
```

468 quadros em 10 s, 46,8 fps, zero perdido.

**Atenção na próxima vez que o binário mudar:** o TCC amarra a
concessão ao hash do executável. Em tese um rebuild a invalida e o
host volta a travar com o mesmo silêncio. Se isso acontecer depois de
um deploy, não procure bug — é a concessão. (Ainda não observado aqui;
anotado como suspeita, não como fato.)

## `setsid` não existe no macOS

A linha de acordar hosts unix era `setsid nohup ...`. No Mac ela morria
em `command not found` **antes** de chegar ao nohup — o host nunca
subia e o sintoma chegava como "chamou VNC". Hoje o `setsid` é
condicional.

## Veja também

- [[windows-locked-screen]] — a mesma fronteira, do lado do Windows
- [[cursor-without-mouse]] — outro caso de "o sintoma não é onde você olha"
