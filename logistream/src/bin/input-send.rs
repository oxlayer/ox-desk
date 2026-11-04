//! Manda alguns eventos de input para o host e sai.
//!
//! Existe para provar o caminho de volta sem depender de alguem mexer o
//! mouse numa janela. O host registra cada evento aceito e grita no
//! recusado, entao o log dele e a prova.
//!
//! **Mexe o cursor da maquina alvo de verdade.** O movimento e pequeno e
//! a tecla e um Shift solto, que nao digita nada.
//!
//! Com `--rajada N` manda N movimentos de mouse SEM pausa entre eles.
//! Serve para medir o que o usuario sentiu: "o video para se eu paro de
//! digitar" e "o input e lento" tinham a mesma causa -- a injecao
//! esperava resposta do Mutter na mesma thread que bombeia o video. Uma
//! rajada enquanto o `sink` mede fps mostra o estrago, e mostra a
//! melhora.
//!
//!   cargo run --release --bin input-send -- <host:porta> [--rajada N]

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

    let rajada: usize = std::env::args()
        .position(|a| a == "--rajada")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // `--rel N`: N deslocamentos relativos de +10 em X.
    //
    // Existe para medir o caminho do KVM sem agarrar o teclado de
    // ninguem. "Parece que nao mexeu" nao e medida: com a posicao do
    // cursor lida antes e depois, ou andou 10*N pixels ou nao andou.
    let rel: usize = std::env::args()
        .position(|a| a == "--rel")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if rel > 0 {
        let mut buf = BytesMut::new();
        for _ in 0..rel {
            buf.clear();
            Evento::MouseRel { dx: 10.0, dy: 0.0 }.escrever(&mut buf);
            envio.write_all(&buf).await?;
            tokio::time::sleep(std::time::Duration::from_millis(8)).await;
        }
        envio.finish()?;
        println!("  {rel} MouseRel de +10 em X = {} px esperados", rel * 10);
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

    // `--capslock`: liga e desliga o Caps Lock da maquina alvo.
    //
    // Teclado precisava de um teste MEDIVEL. "Digitei e nao apareceu"
    // depende de onde estava o foco; o estado do Caps Lock nao depende
    // de nada e se le com uma chamada. Inofensivo: volta como estava.
    if std::env::args().any(|a| a == "--capslock") {
        let mut buf = BytesMut::new();
        // 58 = KEY_CAPSLOCK no evdev, que e o que o protocolo carrega.
        for apertado in [true, false] {
            buf.clear();
            Evento::Tecla { keycode: 58, apertado }.escrever(&mut buf);
            envio.write_all(&buf).await?;
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        envio.finish()?;
        println!("  caps lock alternado uma vez");
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

    if rajada > 0 {
        // Movimento so, sem tecla nem botao: muda o cursor da maquina
        // alvo e nada mais. Um circulo pequeno para o cursor nao parar
        // num canto que dispare coisa do sistema.
        let mut buf = BytesMut::new();
        let t0 = std::time::Instant::now();
        for i in 0..rajada {
            let a = (i as f64) * 0.1;
            let e = Evento::Mouse { x: 1300.0 + 60.0 * a.cos(), y: 750.0 + 60.0 * a.sin() };
            buf.clear();
            e.escrever(&mut buf);
            envio.write_all(&buf).await?;
        }
        envio.finish()?;
        let s = t0.elapsed().as_secs_f64();
        println!("  {rajada} eventos em {s:.2}s = {:.0}/s", rajada as f64 / s);
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

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
