//! Mata a maior incognita da v1: o NVENC funciona nesta maquina, por
//! carregamento em runtime, sem SDK e sem sudo?
//!
//! Codifica quadros SINTETICOS -- captura e outro problema, e misturar os
//! dois faria o fracasso de um parecer o do outro. Se este binario
//! escrever um .h264 que o ffprobe le, a codificacao esta provada e a
//! unica incognita que sobra na v1 e a captura.
//!
//!   cargo run --release --bin nvenc-probe -- [quadros] [largura] [altura]

use std::io::Write;
use std::time::Instant;

use cudarc::driver::CudaContext;
use moq_nvenc::sys::nvEncodeAPI::{
    NV_ENC_BUFFER_FORMAT, NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, NV_ENC_TUNING_INFO,
};
use moq_nvenc::{EncodePictureParams, Encoder, EncoderInitParams};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let n: u64 = a.first().and_then(|v| v.parse().ok()).unwrap_or(120);
    let w: u32 = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(1920);
    let h: u32 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(1080);

    // `load` antes de tocar em CUDA: a crate separa "a biblioteca existe
    // e e nova o bastante" de "a GPU aceitou", e saber qual dos dois
    // falhou importa numa maquina sem driver.
    Encoder::load().map_err(|e| anyhow::anyhow!("NVENC indisponivel: {e:?}"))?;
    println!("  NVENC carregado (dlopen de libnvidia-encode)");

    let ctx = CudaContext::new(0).map_err(|e| anyhow::anyhow!("contexto CUDA: {e:?}"))?;
    let enc = Encoder::initialize_with_cuda(ctx)
        .map_err(|e| anyhow::anyhow!("sessao NVENC: {e:?}"))?;

    let codecs = enc.get_encode_guids().map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("  codecs oferecidos pela GPU: {}", codecs.len());
    anyhow::ensure!(
        codecs.contains(&NV_ENC_CODEC_H264_GUID),
        "esta GPU nao oferece H.264 por hardware"
    );

    // Sem config explicita o bitstream sai SEM SPS/PPS, e o ffprobe
    // recusa com "non-existing PPS 0 referenced". Nao e detalhe de
    // arquivo: um cliente que entra no meio da transmissao -- o caso
    // normal numa tela remota -- precisa dos cabecalhos para comecar a
    // draw. `repeatSPSPPS` os reenvia em cada IDR.
    let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
    let preset = enc
        .get_preset_config(NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, tuning)
        .map_err(|e| anyhow::anyhow!("preset: {e}"))?;
    let mut cfg = preset.presetCfg;
    // Uniao do bindgen: so o codec escolhido e valido ali dentro, e
    // escolhemos H.264 na linha acima.
    unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };

    let mut params = EncoderInitParams::new(NV_ENC_CODEC_H264_GUID, w, h);
    params
        .preset_guid(NV_ENC_PRESET_P4_GUID)
        // Tela remota vive de latencia, nao de bitrate bonito.
        .tuning_info(tuning)
        .framerate(60, 1)
        .enable_picture_type_decision();
    // Seguro: a config veio do proprio `get_preset_config`, cujos
    // ponteiros reservados sao nulos, como a doc da crate pede.
    unsafe { params.encode_config(cfg) };

    let session = enc
        .start_session(NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB, params)
        .map_err(|e| anyhow::anyhow!("iniciar sessao {w}x{h}: {e}"))?;

    let mut entrada = session.create_input_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut saida = session.create_output_bitstream().map_err(|e| anyhow::anyhow!("{e}"))?;

    let destino = std::env::temp_dir().join("logistream-probe.h264");
    let mut arquivo = std::fs::File::create(&destino)?;

    let mut quadro = vec![0u8; (w * h * 4) as usize];
    let mut bytes = 0usize;
    let mut piores = Vec::new();
    let t0 = Instant::now();

    for i in 0..n {
        // Gradiente que anda: conteudo estatico faria o encoder produzir
        // quadros quase vazios e o numero mediria nada.
        pinta(&mut quadro, w, h, i);

        let tq = Instant::now();
        {
            let mut lock = entrada.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
            // Seguro aqui: o buffer foi criado com estas mesmas dimensoes
            // e formato, entao o tamanho confere por construcao.
            unsafe { lock.write(&quadro) };
        }
        let sub = session
            .encode_picture(
                entrada,
                saida,
                EncodePictureParams { input_timestamp: i, force_idr: i == 0 },
            )
            .map_err(|e| anyhow::anyhow!("encode do quadro {i}: {e}"))?;
        let (data, e2, s2) = sub.finish().map_err(|e| anyhow::anyhow!("{e}"))?;
        piores.push(tq.elapsed().as_secs_f64() * 1000.0);
        bytes += data.len();
        arquivo.write_all(&data)?;
        entrada = e2;
        saida = s2;
    }
    arquivo.flush()?;

    let total = t0.elapsed().as_secs_f64();
    piores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = piores[piores.len() / 2];
    let p95 = piores[piores.len() * 95 / 100];
    println!("  {n} quadros {w}x{h} em {total:.2}s = {:.0} fps", n as f64 / total);
    println!("  por quadro: p50 {p50:.2}ms  p95 {p95:.2}ms");
    println!(
        "  {:.1} KB total = {:.1} Mbps a 60fps",
        bytes as f64 / 1024.0,
        (bytes as f64 * 8.0 / n as f64) * 60.0 / 1_000_000.0
    );
    println!("  escrito em {}", destino.display());
    Ok(())
}

/// Gradiente diagonal que desliza com o quadro.
fn pinta(buf: &mut [u8], w: u32, h: u32, i: u64) {
    let desloc = (i * 7) as u32;
    for y in 0..h {
        let linha = (y * w * 4) as usize;
        for x in 0..w {
            let p = linha + (x * 4) as usize;
            buf[p] = ((x + desloc) % 256) as u8;
            buf[p + 1] = ((y + desloc) % 256) as u8;
            buf[p + 2] = ((x + y + desloc) % 256) as u8;
            buf[p + 3] = 255;
        }
    }
}
