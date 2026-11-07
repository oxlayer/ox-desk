//! Decodificacao, com a plataforma escolhendo o caminho.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! | onde | como | por que |
//! |---|---|---|
//! | macOS | `ffmpeg` (libavcodec do brew) | nao tem NVIDIA |
//! | Linux, Windows | NVDEC | a GPU esta ali e o decodificador vem com o driver |
//!
//! **Um por plataforma, nao os dois em todo lugar.** Carregar os dois
//! significaria `libavcodec` instalado no Windows -- que e exactamente o
//! que mantinha aquele lado no RDP. E ter dois caminhos ativos na mesma
//! maquina multiplicaria os modos de falha sem ninguem ganhar nada: o Mac
//! nao tem como usar NVDEC e as outras duas nao precisam do ffmpeg.
//!
//! A fronteira e esta: entra unidade de acesso H.264, sai pixel pronto
//! para a janela, JA no tamanho e na posicao em que vai ser desenhado.
//! Quem decide esse tamanho e a janela, entao ela passa a conta como
//! funcao -- o decodificador e o unico que sabe o tamanho da origem, e
//! sem isso a conta nao fecha.

/// Um quadro pronto para o `Slot` do visualizador.
pub struct DecodedFrame {
    pub rgb: Vec<u32>,
    /// Canto onde a imagem comeca dentro da janela.
    pub ox: u32,
    pub oy: u32,
    /// Tamanho desenhado.
    pub w: u32,
    pub h: u32,
    /// Tamanho da tela remota -- o espaco em que o host fala.
    pub rw: u32,
    pub rh: u32,
}

/// Dado o tamanho da ORIGEM, devolve `(ox, oy, w, h)` do destino.
pub type Fit<'a> = &'a mut dyn FnMut(u32, u32) -> (u32, u32, u32, u32);

#[cfg(target_os = "macos")]
mod interno {
    use super::{Fit, DecodedFrame};

    /// Caminho do Mac: libavcodec decodifica, swscale converte e escala.
    pub struct Decoder {
        dec: ffmpeg_next::decoder::Video,
        escalador: Option<ffmpeg_next::software::scaling::Context>,
        /// (origem_w, origem_h, destino_w, destino_h) do escalador atual.
        /// Recriar custa caro; so quando de fato mudou.
        chave: (u32, u32, u32, u32),
    }

    impl Decoder {
        pub fn new() -> Result<Self, String> {
            ffmpeg_next::init().map_err(|e| format!("ffmpeg: {e}"))?;
            let codec = ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::H264)
                .ok_or("sem decoder H.264")?;
            let ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec);
            let dec = ctx.decoder().video().map_err(|e| format!("decoder: {e}"))?;
            Ok(Decoder { dec, escalador: None, chave: (0, 0, 0, 0) })
        }

        pub fn name(&self) -> &'static str {
            "ffmpeg"
        }

        pub fn feed(&mut self, bytes: &[u8], encaixe: Fit) -> Result<Vec<DecodedFrame>, String> {
            let pacote = ffmpeg_next::Packet::copy(bytes);
            if self.dec.send_packet(&pacote).is_err() {
                // Quadro sem referencia (entrou no meio do fluxo): nao e
                // erro, e so nao da nada. O proximo quadro-chave resolve.
                return Ok(Vec::new());
            }
            let mut saida = Vec::new();
            let mut quadro = ffmpeg_next::frame::Video::empty();
            while self.dec.receive_frame(&mut quadro).is_ok() {
                let (rw, rh) = (quadro.width(), quadro.height());
                let (ox, oy, dw, dh) = encaixe(rw, rh);
                if dw == 0 || dh == 0 {
                    continue;
                }
                if self.escalador.is_none() || self.chave != (rw, rh, dw, dh) {
                    self.escalador = ffmpeg_next::software::scaling::Context::get(
                        quadro.format(),
                        rw,
                        rh,
                        ffmpeg_next::format::Pixel::BGRA,
                        dw,
                        dh,
                        ffmpeg_next::software::scaling::Flags::BILINEAR,
                    )
                    .ok();
                    self.chave = (rw, rh, dw, dh);
                }
                let Some(sc) = self.escalador.as_mut() else { continue };
                let mut bgra = ffmpeg_next::frame::Video::empty();
                if sc.run(&quadro, &mut bgra).is_err() {
                    continue;
                }
                // Copia por LINHA, nao por pixel.
                //
                // Era um laco por pixel com verificacao de limite dentro:
                // 921 mil iteracoes por quadro numa janela de 1280x720, a
                // 60-80 quadros por segundo, para produzir bytes que o
                // `swscale` JA entregou no layout certo. BGRA em bytes e
                // exatamente `0xAARRGGBB` em little-endian, que e o que o
                // softbuffer espera.
                //
                // Entao sao 720 `copy_from_slice` em vez de 921 mil
                // conversoes. O `unsafe` e uma reinterpretacao de
                // `[u32]` como `[u8]`: mesmo tamanho total, e ir de
                // alinhamento 4 para 1 e sempre valido.
                //
                // Little-endian esta assumido aqui e nas duas pontas que
                // nos interessam (x86-64 e Apple Silicon). Num alvo
                // big-endian isto inverteria os canais -- barulhento o
                // bastante para alguem notar na primeira imagem.
                let linha = bgra.stride(0);
                let data = bgra.data(0);
                let largura_bytes = dw as usize * 4;
                let mut rgb = vec![0u32; (dw * dh) as usize];
                {
                    let destino: &mut [u8] = unsafe {
                        std::slice::from_raw_parts_mut(
                            rgb.as_mut_ptr() as *mut u8,
                            rgb.len() * 4,
                        )
                    };
                    for y in 0..dh as usize {
                        let o = y * linha;
                        let d = y * largura_bytes;
                        if o + largura_bytes > data.len() || d + largura_bytes > destino.len() {
                            break;
                        }
                        destino[d..d + largura_bytes]
                            .copy_from_slice(&data[o..o + largura_bytes]);
                    }
                }
                saida.push(DecodedFrame { rgb, ox, oy, w: dw, h: dh, rw, rh });
            }
            Ok(saida)
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
mod interno {
    use super::{Fit, DecodedFrame};
    use crate::decode_nv;

    /// Caminho das maquinas NVIDIA: NVDEC decodifica, e a conversao de
    /// cor ja escala no mesmo passo.
    pub struct Decoder {
        dec: decode_nv::Decoder,
    }

    impl Decoder {
        pub fn new() -> Result<Self, String> {
            Ok(Decoder { dec: decode_nv::Decoder::new()? })
        }

        pub fn name(&self) -> &'static str {
            "nvdec"
        }

        pub fn feed(&mut self, bytes: &[u8], encaixe: Fit) -> Result<Vec<DecodedFrame>, String> {
            let mut saida = Vec::new();
            for q in self.dec.feed(bytes)? {
                let (ox, oy, dw, dh) = encaixe(q.w, q.h);
                if dw == 0 || dh == 0 {
                    continue;
                }
                let mut rgb = vec![0u32; (dw * dh) as usize];
                decode_nv::nv12_to_bgra(&q, &mut rgb, dw, dh);
                saida.push(DecodedFrame { rgb, ox, oy, w: dw, h: dh, rw: q.w, rh: q.h });
            }
            Ok(saida)
        }
    }
}

pub use interno::Decoder;
