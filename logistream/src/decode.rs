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
pub struct Pronto {
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
pub type Encaixe<'a> = &'a mut dyn FnMut(u32, u32) -> (u32, u32, u32, u32);

#[cfg(target_os = "macos")]
mod interno {
    use super::{Encaixe, Pronto};

    /// Caminho do Mac: libavcodec decodifica, swscale converte e escala.
    pub struct Decodificador {
        dec: ffmpeg_next::decoder::Video,
        escalador: Option<ffmpeg_next::software::scaling::Context>,
        /// (origem_w, origem_h, destino_w, destino_h) do escalador atual.
        /// Recriar custa caro; so quando de fato mudou.
        chave: (u32, u32, u32, u32),
    }

    impl Decodificador {
        pub fn novo() -> Result<Self, String> {
            ffmpeg_next::init().map_err(|e| format!("ffmpeg: {e}"))?;
            let codec = ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::H264)
                .ok_or("sem decoder H.264")?;
            let ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec);
            let dec = ctx.decoder().video().map_err(|e| format!("decoder: {e}"))?;
            Ok(Decodificador { dec, escalador: None, chave: (0, 0, 0, 0) })
        }

        pub fn nome(&self) -> &'static str {
            "ffmpeg"
        }

        pub fn alimentar(&mut self, bytes: &[u8], encaixe: Encaixe) -> Result<Vec<Pronto>, String> {
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
                // BGRA em bytes vira 0xAARRGGBB em little-endian, que e o
                // que o softbuffer espera (o byte alto e ignorado).
                let linha = bgra.stride(0);
                let dados = bgra.data(0);
                let mut rgb = vec![0u32; (dw * dh) as usize];
                for y in 0..dh as usize {
                    let ini = y * linha;
                    for x in 0..dw as usize {
                        let p = ini + x * 4;
                        if p + 3 < dados.len() {
                            rgb[y * dw as usize + x] = u32::from_le_bytes([
                                dados[p],
                                dados[p + 1],
                                dados[p + 2],
                                dados[p + 3],
                            ]);
                        }
                    }
                }
                saida.push(Pronto { rgb, ox, oy, w: dw, h: dh, rw, rh });
            }
            Ok(saida)
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
mod interno {
    use super::{Encaixe, Pronto};
    use crate::decode_nv;

    /// Caminho das maquinas NVIDIA: NVDEC decodifica, e a conversao de
    /// cor ja escala no mesmo passo.
    pub struct Decodificador {
        dec: decode_nv::Decodificador,
    }

    impl Decodificador {
        pub fn novo() -> Result<Self, String> {
            Ok(Decodificador { dec: decode_nv::Decodificador::novo()? })
        }

        pub fn nome(&self) -> &'static str {
            "nvdec"
        }

        pub fn alimentar(&mut self, bytes: &[u8], encaixe: Encaixe) -> Result<Vec<Pronto>, String> {
            let mut saida = Vec::new();
            for q in self.dec.alimentar(bytes)? {
                let (ox, oy, dw, dh) = encaixe(q.w, q.h);
                if dw == 0 || dh == 0 {
                    continue;
                }
                let mut rgb = vec![0u32; (dw * dh) as usize];
                decode_nv::nv12_para_bgra(&q, &mut rgb, dw, dh);
                saida.push(Pronto { rgb, ox, oy, w: dw, h: dh, rw: q.w, rh: q.h });
            }
            Ok(saida)
        }
    }
}

pub use interno::Decodificador;
