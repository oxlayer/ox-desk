# Mandar não é ter chegado; e número que ninguém imprime ninguém confere

Duas faces da mesma regra, as duas pagas caro.

## 1. Todo passo remoto confere o que chegou

Um `scp` que "deu certo" não garante o arquivo certo no destino. O
Defender já **restaurou a cópia em quarentena por cima** do
`logid-agent.exe` recém-enviado — 402984 bytes onde 458792 tinham sido
mandados. O deploy dizia ok.

Por isso cada passo do Windows no `deploy.sh` captura stderr, testa o
resultado e confere o tamanho do arquivo que aterrissou **no
PowerShell**, não no bash (onde a expansão aconteceria do lado errado).

Mesma família: `cargo test` **não** constrói o entregável. Rodar os
testes e instalar achando que o binário foi atualizado já custou uma
hora de confusão sobre por que a validação não estava valendo.

## 2. Número que importa tem de aparecer no log

O alvo de bitrate do NVENC ficou errado por semanas — a fórmula caía no
piso de 8 Mbps em qualquer resolução enquanto o comentário prometia 15
e 34. Sobreviveu porque **só o host do Mac imprimia o alvo**. Quando a
tela Retina também anunciou "8 Mbps", o número ficou absurdo o bastante
para alguém refazer a conta.

O host do Windows não imprimia nada. Hoje os três imprimem.

Mesma história do custo da conversão NV12→BGRA: 28 ms por quadro, teto
de 35 fps, segurando o desenho em 27 enquanto a rede entregava 60.
Ninguém tinha medido porque "é só um laço". Medir custou um teste e
apontou direto; hoje é teste de orçamento que quebra o build.

**Se uma proteção, um alvo ou um custo não aparece em lugar nenhum,
assuma que está errado.** Ver também a flag `dispatch.backfill_gate` no
outro repo: existia, estava documentada, estava ligada, e não era lida
por código nenhum.

## Veja também

- [[powershell-remoto]] — como mandar sem quebrar
- [[reiniciar-agente]] — reiniciar sem conferir é meio trabalho
