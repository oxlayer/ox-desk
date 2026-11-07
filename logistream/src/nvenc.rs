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
pub struct NvencEncoder {
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
    last_was_idr: bool,
    /// Bitrate medio pedido, em bits/s. Guardado so para o host poder
    /// anuncia-lo no log -- o alvo errado de 8 Mbps sobreviveu semanas
    /// porque o host Windows nunca o imprimiu, e so apareceu no Mac,
    /// que imprimia. Numero que ninguem ve e numero que ninguem confere.
    alvo: u32,
}

impl NvencEncoder {
    /// Bitrate medio pedido, em Mbps.
    pub fn target_mbps(&self) -> f64 {
        self.alvo as f64 / 1_000_000.0
    }

    pub fn new(w: u32, h: u32) -> anyhow::Result<Self> {
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
        // normal -- nao tem cabecalho para comecar a draw.
        unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };

        // --- qualidade ---------------------------------------------------
        //
        // O preset sozinho escolhia ~2 Mbps a 1440p, e o usuario viu o
        // resultado: "a imagem esta meio bugada, sem muita qualidade".
        // Numa LAN isso e banda sobrando na mesa -- o Parsec usa 20-50.
        // A conta mora em `qualidade.rs`, nao aqui: ela estava
        // duplicada no host do Mac e duas copias da mesma formula e
        // como o alvo errado de 8 Mbps sobreviveu semanas.
        let alvo = crate::quality::target(w, h);
        // ~8 Mbps em 1080p, ~15 em 1440p, ~33 em 4K: escala com a area,
        // porque 1440p e 4K nao cabem no mesmo teto.
        //
        // Era `pixels * 60 / 1000`, que da 221 kbps em 1440p e **sempre**
        // caia no piso de 8 Mbps -- em qualquer resolucao. O comentario
        // prometia 15 e 34 e entregava 8 nos tres casos; so apareceu
        // quando a tela Retina do Mac (3360x2100) tambem anunciou
        // "8 Mbps alvo" e o numero ficou absurdo o suficiente para eu
        // conferir a conta.
        //
        // 4 bits por pixel por segundo a 60 fps e o que produz os numeros
        // que o comentario sempre afirmou.

        cfg.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_VBR;
        cfg.rcParams.averageBitRate = alvo;
        // Teto com folga: tela parada gasta quase nada, e quando a
        // imagem muda inteira (rolar uma pagina) e a hora de gastar.
        cfg.rcParams.maxBitRate = alvo * 2;
        // VBV de um quadro, medido no teto e nao na media.
        //
        // Buffer grande deixa o encoder "dever" bits e entregar quadro
        // atrasado, o oposto do que tela remota quer -- por isso um
        // quadro, nao mais. Mas era `alvo / 60`, dimensionado pela
        // media, o que tornava o `maxBitRate` acima pura ficcao: nenhum
        // quadro podia passar de 16 KB, entao a folga do dobro nunca
        // pode ser gasta. Na hora de rolar a tela, que e quando a pessoa
        // esta olhando, o encoder nao tinha permissao de gastar e subia
        // o QP ate `maxQP`. Era isso que aparecia como borrao.
        //
        // A prova estava no log o tempo todo: quadro-chave de 120 KB
        // contra um VBV de 16 KB.
        cfg.rcParams.vbvBufferSize = cfg.rcParams.maxBitRate / 60;
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
        Ok(NvencEncoder {
            sessao: Some(s),
            entrada: Some(entrada),
            saida: Some(saida),
            w,
            h,
            n: 0,
            idr: false,
            last_was_idr: false,
            alvo,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Codifica um quadro BGRA. `stride` e o passo de linha da ORIGEM --
    /// tanto o PipeWire quanto o DXGI entregam linhas com folga no fim, e
    /// quase nunca igual ao passo que o NVENC quer.
    pub fn encode(&mut self, px: &[u8], stride: usize) -> anyhow::Result<Vec<u8>> {
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
                let junto = reempacotar(px, stride, linha, pitch, self.h as usize);
                unsafe { lock.write(&junto) };
            }
        }

        let n = self.n;
        // O primeiro quadro, e todo quadro pedido: sem IDR o cliente que
        // acabou de entrar nao tem como comecar a draw, e com GOP de
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
            Ok((data, i2, s2)) => {
                self.entrada = Some(i2);
                self.saida = Some(s2);
                self.n += 1;
                self.last_was_idr = forcar;
                Ok(data)
            }
            Err(e) => Err(anyhow::anyhow!("encode: {e}")),
        }
    }

    /// Pede que o PROXIMO quadro saia IDR.
    ///
    /// Quem chama e a rede, quando um cliente conecta: ele nao tem
    /// contexto nenhum, e um quadro P sem referencia nao decodifica. Sem
    /// isto a janela abria preta ate o IDR seguinte do GOP -- ate 2s.
    pub fn request_idr(&mut self) {
        self.idr = true;
    }

    /// O ultimo quadro codificado era quadro-chave.
    ///
    /// E o que vai no cabecalho da fatia, e o cliente usa para saber se
    /// pode comecar a draw dali.
    pub fn last_was_idr(&self) -> bool {
        self.last_was_idr
    }
}

/// Copia as linhas da origem para o passo que o NVENC pediu.
///
/// O destino tem `pitch` bytes por linha, e nao `linha`: colar as linhas
/// embaralhava a imagem em toda largura cujo passo tem preenchimento,
/// como 800 px (3200 bytes). Em 1920 e 2560 os passos coincidem, e por
/// isso o defeito so apareceu com a tela virtual do Windows.
fn reempacotar(px: &[u8], stride: usize, linha: usize, pitch: usize, h: usize) -> Vec<u8> {
    let mut destino = vec![0u8; pitch * h];
    for y in 0..h {
        let ini = y * stride;
        if ini + linha > px.len() {
            break;
        }
        destino[y * pitch..y * pitch + linha].copy_from_slice(&px[ini..ini + linha]);
    }
    destino
}

#[cfg(test)]
mod testes_passo {
    #[test]
    fn cada_linha_comeca_no_passo_do_destino() {
        // Tres papeis com valores diferentes: origem, largura e destino.
        let (stride, linha, pitch, h) = (6, 4, 8, 2);
        let px = [1, 2, 3, 4, 9, 9, 5, 6, 7, 8, 9, 9];
        let d = super::reempacotar(&px, stride, linha, pitch, h);
        assert_eq!(d.len(), pitch * h);
        assert_eq!(&d[0..4], &[1, 2, 3, 4]);
        assert_eq!(&d[8..12], &[5, 6, 7, 8]);
    }
}
