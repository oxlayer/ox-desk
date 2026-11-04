//! Manda alguns eventos de input para o host e sai.
//!
//! Existe para provar o caminho de volta sem depender de alguem mexer o
//! mouse numa janela. O host registra cada evento aceito e grita no
//! recusado, entao o log dele e a prova.
//!
//! **Mexe o cursor da maquina alvo de verdade.** O movimento e pequeno e
//! a tecla e um Shift solto, que nao digita nada.
//!
//!   cargo run --release --bin input-send -- <host:porta>

use bytes::BytesMut;
use logistream::input::{Evento, BTN_LEFT};
use logistream::net;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let alvo = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:9777".into());
    let mut enderecos: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&alvo).await?.collect();
    enderecos.sort_by_key(|a| !a.is_ipv4());
    let destino = *enderecos.first().ok_or_else(|| anyhow::anyhow!("nao resolvi {alvo}"))?;
    let bind: std::net::SocketAddr =
        if destino.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let ep = net::cliente(bind)?;
    let conexao = ep.connect(destino, "logistream")?.await?;
    let mut envio = conexao.open_uni().await?;
    println!("  conectado; mandando eventos");

    let mut eventos = Vec::new();
    // Um quadrado pequeno perto do centro: visivel, sem atravessar a
    // tela nem cair em canto que dispara coisa do sistema.
    for (x, y) in [(1200.0, 700.0), (1400.0, 700.0), (1400.0, 800.0), (1200.0, 800.0)] {
        eventos.push(Evento::Mouse { x, y });
    }
    // Shift: aperta e solta sem digitar nada.
    eventos.push(Evento::Tecla { keycode: 42, apertado: true });
    eventos.push(Evento::Tecla { keycode: 42, apertado: false });
    // Rolagem de um passo, que nao destroi nada.
    eventos.push(Evento::Rolagem { dx: 0.0, dy: 15.0 });
    // Botao NAO e enviado: clicar as cegas na maquina de alguem pode
    // apertar qualquer coisa que estiver sob o cursor.
    let _ = BTN_LEFT;

    let mut buf = BytesMut::new();
    for e in &eventos {
        buf.clear();
        e.escrever(&mut buf);
        envio.write_all(&buf).await?;
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    envio.finish()?;
    // Deixa o host consumir antes de fechar a conexao.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    println!("  {} eventos enviados", eventos.len());
    Ok(())
}
