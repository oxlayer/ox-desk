//! Decodifica um arquivo H.264 pelo NVDEC e relata o que saiu.
//!
//! Spec: ../../specs/010-remote-desktop.md
//!
//! Existe para provar o caminho de decodificacao SEM janela, sem rede e
//! sem o visualizador no meio: se este binario decodifica o arquivo que o
//! `logistream-sink` gravou, o NVDEC esta certo e qualquer problema
//! restante e de quem desenha.
//!
//! Grava o ultimo quadro em PPM para poder ser comparado com o mesmo
//! quadro decodificado pelo ffmpeg -- e assim conferir a matriz de cor,
//! que erra sem dar erro.
//!
//!   logistream-probe-nvdec <arquivo.h264> [saida.ppm]

#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    use std::io::Write;
    use std::time::Instant;

    let a: Vec<String> = std::env::args().skip(1).collect();
    let caminho = a.first().cloned().unwrap_or_else(|| "/tmp/logistream-sink.h264".into());
    let ppm = a.get(1).cloned();

    let bytes = std::fs::read(&caminho)?;
    println!("  {} ({} KB)", caminho, bytes.len() / 1024);

    let mut dec = logistream::decode_nv::Decoder::new()
        .map_err(|e| anyhow::anyhow!("NVDEC indisponivel: {e}"))?;
    println!("  NVDEC carregado");

    // Em pedacos, como viria da rede: alimentar o arquivo inteiro de uma
    // vez funcionaria aqui e escondeia o caso real.
    const PEDACO: usize = 64 * 1024;
    let t0 = Instant::now();
    let mut n = 0u64;
    let mut ultimo = None;
    for parte in bytes.chunks(PEDACO) {
        for q in dec.feed(parte).map_err(|e| anyhow::anyhow!(e))? {
            n += 1;
            ultimo = Some(q);
        }
    }
    let s = t0.elapsed().as_secs_f64();
    let (w, h) = dec.size();
    println!(
        "  {n} quadros {w}x{h} em {s:.2}s = {:.0} fps  ({:.2} ms por quadro)",
        n as f64 / s,
        s * 1000.0 / n.max(1) as f64
    );

    let Some(q) = ultimo else {
        anyhow::bail!("nenhum quadro decodificado");
    };

    // Estatisticas em vez de "parece certo": media por canal pega imagem
    // preta, invertida ou com a matriz de cor errada sem precisar de olho.
    let mut rgb = vec![0u32; (q.w * q.h) as usize];
    logistream::decode_nv::nv12_to_bgra(&q, &mut rgb, q.w, q.h);
    let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
    for p in &rgb {
        sr += ((p >> 16) & 0xff) as u64;
        sg += ((p >> 8) & 0xff) as u64;
        sb += (p & 0xff) as u64;
    }
    let np = rgb.len() as u64;
    println!("  media R={} G={} B={}", sr / np, sg / np, sb / np);

    if let Some(p) = ppm {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&p)?);
        write!(f, "P6\n{} {}\n255\n", q.w, q.h)?;
        for px in &rgb {
            f.write_all(&[((px >> 16) & 0xff) as u8, ((px >> 8) & 0xff) as u8, (*px & 0xff) as u8])?;
        }
        println!("  ultimo quadro em {p}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn main() {
    eprintln!("sem NVDEC no macOS: aqui o visualizador decodifica por ffmpeg");
}
