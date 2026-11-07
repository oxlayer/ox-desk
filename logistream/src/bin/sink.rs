//! Receptor: QUIC -> quadros -> arquivo. A v2 do lado de la.
//!
//! Ainda nao desenha nada -- decodificar e render sao a proxima etapa.
//! O que este binario responde e se o **transporte** aguenta: quantos
//! quadros chegam inteiros, quantos se perdem, e com que atraso.
//!
//! O relogio nao e o mesmo nas duas maquinas, entao a latencia so vale
//! em loopback. Entre maquinas o numero vira o desvio dos relogios mais
//! a rede, e e dito assim em vez de apresentado como verdade.
//!
//! Com `--rajada N` ele tambem manda N movimentos de mouse pela MESMA
//! conexao, como o visualizador faz. Isso importa: o host serve um
//! cliente por vez, entao medir input e video por conexoes separadas nao
//! reproduz nada -- a segunda fica na fila. Foi assim que a primeira
//! tentativa de medir "o video para quando eu digito" mediu nada.
//!
//!   cargo run --release --bin sink -- <host:porta> [segundos] [--rajada N]

use std::io::Write;
use std::time::{Duration, Instant};

use logistream::net::{self, Reassembler};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let alvo = a.first().cloned().unwrap_or_else(|| "127.0.0.1:9777".into());
    let segundos: u64 = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(10);

    // Resolver TODOS e preferir IPv4, e nao pegar o primeiro.
    //
    // `ubuntu-host.local` resolve nas duas familias, e o Mac devolve o
    // IPv6 primeiro. Com o socket preso em `0.0.0.0`, o quinn recusa com
    // "invalid remote address" -- a mesma armadilha que o `ssh` do
    // daemon ja tinha dado. O socket local tem de nascer da familia do
    // destino.
    let mut enderecos: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host(&alvo).await?.collect();
    enderecos.sort_by_key(|a| !a.is_ipv4());
    let destino = *enderecos
        .first()
        .ok_or_else(|| anyhow::anyhow!("nao resolvi {alvo}"))?;
    let bind: std::net::SocketAddr =
        if destino.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let ep = net::client(bind)?;
    println!("  conectando em {destino}");
    let conexao = ep.connect(destino, "logistream")?.await?;
    println!("  conectado");

    // Rajada de input pela MESMA conexao, antes de comecar a medir.
    let rajada: usize = a
        .iter()
        .position(|x| x == "--rajada")
        .and_then(|i| a.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if rajada > 0 {
        use bytes::BytesMut;
        use logistream::input::Event;
        let mut envio = conexao.open_uni().await?;
        let mut buf = BytesMut::new();
        for i in 0..rajada {
            let ang = (i as f64) * 0.1;
            let e = Event::Mouse {
                x: 1300.0 + 60.0 * ang.cos(),
                y: 750.0 + 60.0 * ang.sin(),
            };
            buf.clear();
            e.write(&mut buf);
            envio.write_all(&buf).await?;
        }
        println!("  {rajada} eventos de input enviados pela mesma conexao");
    }

    let caminho = std::env::temp_dir().join("logistream-sink.h264");
    let mut arquivo = std::fs::File::create(&caminho)?;

    let mut r = Reassembler::default();
    let mut quadros = 0u64;
    let mut bytes = 0u64;
    let mut datagramas = 0u64;
    let mut atrasos_ms: Vec<f64> = Vec::new();
    let t0 = Instant::now();
    let fim = t0 + Duration::from_secs(segundos);

    while Instant::now() < fim {
        let resto = fim.saturating_duration_since(Instant::now());
        let d = match tokio::time::timeout(resto, conexao.read_datagram()).await {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => {
                println!("  conexao encerrada: {e}");
                break;
            }
            Err(_) => break,
        };
        datagramas += 1;
        bytes += d.len() as u64;
        if let Some((cab, quadro)) = r.push(&d) {
            let agora = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|x| x.as_nanos() as u64)
                .unwrap_or(0);
            atrasos_ms.push((agora.saturating_sub(cab.captured_at_ns)) as f64 / 1e6);
            arquivo.write_all(&quadro)?;
            quadros += 1;
        }
    }
    arquivo.flush()?;

    let s = t0.elapsed().as_secs_f64();
    let pct = |v: &Vec<f64>, q: usize| {
        let mut x = v.clone();
        x.sort_by(|a, b| a.partial_cmp(b).unwrap());
        x.get(x.len().saturating_mul(q) / 100).copied().unwrap_or(0.0)
    };
    println!("\n  {quadros} quadros inteiros em {s:.1}s = {:.1} fps", quadros as f64 / s.max(0.001));
    println!("  {datagramas} datagramas, {:.1} Mbps", bytes as f64 * 8.0 / s / 1_000_000.0);
    println!("  quadros perdidos (chegaram incompletos): {}", r.lost);
    if !atrasos_ms.is_empty() {
        println!(
            "  captura -> quadro montado: p50 {:.1}ms  p95 {:.1}ms  (so vale em loopback: \
             entre maquinas isto inclui o desvio dos relogios)",
            pct(&atrasos_ms, 50),
            pct(&atrasos_ms, 95)
        );
    }
    println!("  {}", caminho.display());
    Ok(())
}
