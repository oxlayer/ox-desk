//! NVENC: quadro BGRA -> H.264, pela GPU.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! Mora na biblioteca porque os dois hosts sao NVIDIA (RTX 3060 no
//! Ubuntu, RTX 3050 no Windows) e a `moq-nvenc` carrega a biblioteca em
//! runtime nos dois (`libnvidia-encode.so.1` e `nvEncodeAPI64.dll`).
//! Entao e um caminho de codificacao, nao dois -- e os parametros de
//! qualidade, que custaram uma rodada de "a imagem esta bugada", ficam
//! num lugar so.

use cudarc::driver::CudaContext;
use moq_nvenc::sys::nvEncodeAPI::{
    NV_ENC_BUFFER_FORMAT, NV_ENC_CODEC_H264_GUID, NV_ENC_PARAMS_RC_MODE, NV_ENC_PRESET_P4_GUID,
    NV_ENC_TUNING_INFO,
};
use moq_nvenc::{EncodePictureParams, Encoder, EncoderInitParams};

/// Sessao de codificacao com os buffers dela.
///
/// Os `Option` existem porque a API consome os buffers no
/// `encode_picture` e os devolve no `finish` -- esta struct e o lugar
/// onde esse vai-e-volta fica escondido, em vez de aparecer no laco de
/// captura de cada plataforma.
pub struct Codificador {
    sessao: Option<moq_nvenc::Session>,
    entrada: Option<moq_nvenc::Buffer>,
    saida: Option<moq_nvenc::Bitstream>,
    w: u32,
    h: u32,
    /// Numero do quadro. O zero sai IDR a forca; os demais seguem o GOP.
    n: u64,
    /// Proximo quadro sai IDR custe o que custar.
    idr: bool,
    /// O que saiu da ultima codificacao foi IDR.
    ultimo_foi_idr: bool,
}

impl Codificador {
    pub fn novo(w: u32, h: u32) -> anyhow::Result<Self> {
        Encoder::load().map_err(|e| anyhow::anyhow!("NVENC: {e:?}"))?;
        let ctx = CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA: {e:?}"))?;
        let enc =
            Encoder::initialize_with_cuda(ctx).map_err(|e| anyhow::anyhow!("sessao: {e:?}"))?;
        let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
        let preset = enc
            .get_preset_config(NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, tuning)
            .map_err(|e| anyhow::anyhow!("preset: {e}"))?;
        let mut cfg = preset.presetCfg;
        // Sem isto, o cliente que entra no meio da transmissao -- o caso
        // normal -- nao tem cabecalho para comecar a desenhar.
        unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };

        // --- qualidade ---------------------------------------------------
        //
        // O preset sozinho escolhia ~2 Mbps a 1440p, e o usuario viu o
        // resultado: "a imagem esta meio bugada, sem muita qualidade".
        // Numa LAN isso e banda sobrando na mesa -- o Parsec usa 20-50.
        let pixels = (w as u64) * (h as u64);
        // ~8 Mbps em 1080p, ~15 em 1440p, ~34 em 4K. Escala com a area
        // em vez de ser numero fixo: 1440p e 4K nao cabem no mesmo teto.
        let alvo = (pixels * 60 / 1000).clamp(8_000_000, 60_000_000) as u32;
        cfg.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_VBR;
        cfg.rcParams.averageBitRate = alvo;
        // Teto com folga: tela parada gasta quase nada, e quando a
        // imagem muda inteira (rolar uma pagina) e a hora de gastar.
        cfg.rcParams.maxBitRate = alvo * 2;
        // VBV de um quadro: buffer grande deixa o encoder "dever" bits e
        // entregar quadro atrasado, o oposto do que tela remota quer.
        cfg.rcParams.vbvBufferSize = alvo / 60;
        // Piso de qualidade: `maxQP` limita o PIOR quadro. Sem ele, numa
        // cena que muda inteira o encoder joga a qualidade no chao para
        // caber no bitrate, e e justamente ali que a pessoa esta olhando.
        cfg.rcParams.set_enableMaxQP(1);
        cfg.rcParams.maxQP.qpIntra = 28;
        cfg.rcParams.maxQP.qpInterP = 30;
        cfg.rcParams.maxQP.qpInterB = 32;

        // --- recuperacao de erro -----------------------------------------
        //
        // So havia IDR no quadro zero. Perdeu um quadro, o erro NUNCA
        // cicatrizava -- sujava a imagem para sempre. Um IDR a cada 2s
        // limita o estrago a 2s.
        cfg.gopLength = 120;
        // Sem quadros B: melhoram compressao a custo de atrasar a saida,
        // e aqui latencia vale mais que bitrate.
        cfg.frameIntervalP = 1;
        // Escrever em campo de uniao e seguro; so ler exige `unsafe`.
        cfg.encodeCodecConfig.h264Config.idrPeriod = 120;

        let mut params = EncoderInitParams::new(NV_ENC_CODEC_H264_GUID, w, h);
        params
            .preset_guid(NV_ENC_PRESET_P4_GUID)
            .tuning_info(tuning)
            .framerate(60, 1)
            .enable_picture_type_decision();
        unsafe { params.encode_config(cfg) };
        let s = enc
            .start_session(NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB, params)
            .map_err(|e| anyhow::anyhow!("start: {e}"))?;
        let entrada = s.create_input_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
        let saida = s.create_output_bitstream().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Codificador {
            sessao: Some(s),
            entrada: Some(entrada),
            saida: Some(saida),
            w,
            h,
            n: 0,
            idr: false,
            ultimo_foi_idr: false,
        })
    }

    pub fn tamanho(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Codifica um quadro BGRA. `stride` e o passo de linha da ORIGEM --
    /// tanto o PipeWire quanto o DXGI entregam linhas com folga no fim, e
    /// quase nunca igual ao passo que o NVENC quer.
    pub fn codificar(&mut self, px: &[u8], stride: usize) -> anyhow::Result<Vec<u8>> {
        let (sessao, mut entrada, saida) = match (
            self.sessao.take(),
            self.entrada.take(),
            self.saida.take(),
        ) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            // Um erro anterior deixou a sessao sem buffer: nao ha como
            // seguir, e devolver o que sobrou mantem o estado coerente.
            (a, b, c) => {
                self.sessao = a;
                self.entrada = b;
                self.saida = c;
                anyhow::bail!("codificador sem buffers");
            }
        };

        {
            let mut lock = match entrada.lock() {
                Ok(l) => l,
                Err(e) => {
                    self.sessao = Some(sessao);
                    self.saida = Some(saida);
                    return Err(anyhow::anyhow!("lock: {e}"));
                }
            };
            let pitch = lock.pitch() as usize;
            let linha = (self.w as usize) * 4;
            if pitch == stride {
                let tam = (stride * self.h as usize).min(px.len());
                unsafe { lock.write(&px[..tam]) };
            } else {
                // Reempacota linha a linha. Alocar aqui a cada quadro
                // seria 14 MB/quadro de churn, entao o buffer vive na
                // struct -- mas ele depende do lock, que e por chamada.
                let mut junto = Vec::with_capacity(linha * self.h as usize);
                for y in 0..self.h as usize {
                    let ini = y * stride;
                    if ini + linha <= px.len() {
                        junto.extend_from_slice(&px[ini..ini + linha]);
                    }
                }
                unsafe { lock.write(&junto) };
            }
        }

        let n = self.n;
        // O primeiro quadro, e todo quadro pedido: sem IDR o cliente que
        // acabou de entrar nao tem como comecar a desenhar, e com GOP de
        // 2s ele ficaria preto esse tempo todo.
        let forcar = n == 0 || self.idr;
        self.idr = false;
        let r = sessao.encode_picture(
            entrada,
            saida,
            EncodePictureParams { input_timestamp: n, force_idr: forcar },
        );
        self.sessao = Some(sessao);
        match r.and_then(|s| s.finish()) {
            Ok((dados, i2, s2)) => {
                self.entrada = Some(i2);
                self.saida = Some(s2);
                self.n += 1;
                self.ultimo_foi_idr = forcar;
                Ok(dados)
            }
            Err(e) => Err(anyhow::anyhow!("encode: {e}")),
        }
    }

    /// Pede que o PROXIMO quadro saia IDR.
    ///
    /// Quem chama e a rede, quando um cliente conecta: ele nao tem
    /// contexto nenhum, e um quadro P sem referencia nao decodifica. Sem
    /// isto a janela abria preta ate o IDR seguinte do GOP -- ate 2s.
    pub fn pedir_idr(&mut self) {
        self.idr = true;
    }

    /// O ultimo quadro codificado era quadro-chave.
    ///
    /// E o que vai no cabecalho da fatia, e o cliente usa para saber se
    /// pode comecar a desenhar dali.
    pub fn ultimo_foi_idr(&self) -> bool {
        self.ultimo_foi_idr
    }
}
