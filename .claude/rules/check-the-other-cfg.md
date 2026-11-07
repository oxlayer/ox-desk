# Código de outro sistema compila aqui — troque o `cfg` e confira

`setup_mac.rs` e `setup_win.rs` só entram no build da própria
plataforma (`#[cfg(target_os = ...)]` em `main.rs`). A consequência é
que **editar o código do Mac daqui não prova nada**: `cargo build`
passa verde porque o arquivo nem foi lido.

Para o Windows isso nunca foi problema — existe alvo cruzado:

```bash
cargo check --target x86_64-pc-windows-gnu --manifest-path logid/Cargo.toml
```

Para o Mac não há SDK aqui. Mas a maior parte desses módulos é `std`
puro: quem fala com o sistema são `Command::new("/usr/bin/security")`,
`launchctl`, `codesign` — strings, não API nativa. Então dá para
type-checar trocando o `cfg` por um minuto:

```bash
cp logid/src/main.rs /tmp/main.rs.bak
sed -i 's/#\[cfg(target_os = "macos")\]\nmod setup_mac;/#[allow(dead_code)]\nmod setup_mac;/' logid/src/main.rs
cargo check --manifest-path logid/Cargo.toml
cp /tmp/main.rs.bak logid/src/main.rs
```

Foi assim que o `setup_mac.rs` da [`015`](../../specs/015-ota.md) foi
verificado com o MacBook fora do ar o dia inteiro: zero erro, zero
aviso. Se houvesse um `?` no lugar errado ou um argumento a mais numa
chamada, eu teria descoberto **depois** de mandar o deploy e esperar
sete minutos de compilação remota — e o deploy diria `FALHOU` numa
linha que não explica nada.

## O que isso NÃO cobre

- API nativa de verdade (FFI de `CoreGraphics`, `getuid`): essa não
  compila aqui, e se o módulo tiver muita, o truque não serve.
- Comportamento. Type-check diz que a chamada existe, não que o
  `launchctl` aceita aquele argumento.

Ou seja: continua valendo "não afirme que funciona no Mac sem ter
rodado lá". O que a troca de `cfg` elimina é a classe mais boba de
erro, que é justamente a que custa a viagem de ida e volta.

## Veja também

- [[verify-what-arrived]] — compilar não é ter chegado
- [[graphical-session]] — o que não nasce por SSH, e por isso não se testa daqui
