//! Decodificacao por NVDEC: H.264 -> NV12 -> BGRA, na GPU.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Por que existe, se o `ffmpeg-next` ja decodifica
//!
//! Porque o `libavcodec` precisa estar instalado, e isso bloqueou o
//! ultimo caminho que ainda usava RDP: nao ha `libavcodec` no Windows, e
//! levar as DLLs para la (ou montar compilacao cruzada com os headers da
//! FFmpeg) e encanamento que nao melhora nada. As duas maquinas que
//! assistem uma a outra tem GPU NVIDIA, e o decodificador ja vem com o
//! driver -- a mesma `moq-nvenc` que usamos para codificar expoe os
//! pontos de entrada do NVDEC, carregados em runtime.
//!
//! O Mac continua no `ffmpeg`: nao tem NVIDIA. Entao sao dois caminhos de
//! decodificacao de proposito, escolhidos por plataforma, e nao um
//! caminho torto que serve mal aos dois.
//!
//! ## Como o CUVID quer ser usado
//!
//! Nao e "me de um pacote, devolva um quadro". E um parser com tres
//! callbacks em C, chamados de dentro do `cuvidParseVideoData`:
//!
//! ```text
//!   sequencia  -> o fluxo se apresentou; crie o decodificador
//!   decodifica -> ha quadro pronto para decodificar; mande
//!   exibe      -> ha quadro pronto para mostrar; mapeie e copie
//! ```
//!
//! Por isso o estado mora num `Box` cujo ponteiro vai como `pUserData`:
//! os callbacks sao `extern "C"` e nao capturam nada.

use std::ffi::c_void;
use std::sync::Arc;

use cudarc::driver::CudaContext;
use moq_nvenc::cuvid;
use moq_nvenc::sys::cuviddec::{
    cudaVideoChromaFormat_enum, cudaVideoCodec_enum, cudaVideoCreateFlags_enum,
    cudaVideoDeinterlaceMode_enum, cudaVideoSurfaceFormat_enum, CUvideodecoder,
    CUVIDDECODECREATEINFO, CUVIDPICPARAMS, CUVIDPROCPARAMS,
};
use moq_nvenc::sys::nvcuvid::{
    CUvideoparser, CUVIDEOFORMAT, CUVIDPARSERDISPINFO, CUVIDPARSERPARAMS, CUVIDSOURCEDATAPACKET,
};

/// Um quadro decodificado, ainda em NV12, ja na memoria da CPU.
pub struct Nv12 {
    pub dados: Vec<u8>,
    /// Passo de linha. O NVDEC alinha, entao quase nunca e igual a largura.
    pub pitch: usize,
    pub w: u32,
    pub h: u32,
}

/// O que os callbacks em C compartilham com o dono.
///
/// Nao e `Send`: o contexto CUDA e por thread, e os callbacks sao
/// chamados de dentro do `parse` -- na mesma thread, sempre.
struct Estado {
    decoder: CUvideodecoder,
    /// Tamanho CODIFICADO (multiplo de 16); o de exibicao sai do
    /// `display_area` e e o que vale para o usuario.
    w: u32,
    h: u32,
    /// Quadros prontos. Fila curta: quadro velho numa tela remota nao
    /// tem valor, entao o dono drena a cada passada e o resto se perde.
    prontos: Vec<Nv12>,
    /// Erro de dentro do callback, para o dono ver. Callback em C nao
    /// pode propagar `Result` nem desenrolar panico.
    erro: Option<String>,
}

pub struct Decodificador {
    parser: CUvideoparser,
    estado: Box<Estado>,
    /// Mantem o contexto vivo enquanto o decodificador existir.
    _ctx: Arc<CudaContext>,
}

/// Erro do CUDA/CUVID vira texto com o nome da chamada que falhou --
/// "cuvidCreateDecoder: 801" diz onde olhar; "erro 801" nao.
fn checa(nome: &str, r: cudarc::driver::sys::CUresult) -> Result<(), String> {
    if r == cudarc::driver::sys::CUresult::CUDA_SUCCESS {
        Ok(())
    } else {
        Err(format!("{nome}: {r:?}"))
    }
}

impl Decodificador {
    pub fn novo() -> Result<Self, String> {
        let api = cuvid::Api::get()?;
        let ctx = CudaContext::new(0).map_err(|e| format!("CUDA: {e:?}"))?;

        let mut estado = Box::new(Estado {
            decoder: std::ptr::null_mut(),
            w: 0,
            h: 0,
            prontos: Vec::new(),
            erro: None,
        });

        let mut p: CUVIDPARSERPARAMS = unsafe { std::mem::zeroed() };
        p.CodecType = cudaVideoCodec_enum::cudaVideoCodec_H264;
        // Teto de superficies de decodificacao. O callback de sequencia
        // devolve o minimo que o fluxo exige; este numero so precisa ser
        // >= aquele.
        p.ulMaxNumDecodeSurfaces = 8;
        // Zero: queremos o quadro assim que sai, sem reordenar para
        // apresentacao. Numa tela remota nao ha B-frames (o host desliga)
        // e atraso de exibicao e exatamente o que nao queremos.
        p.ulMaxDisplayDelay = 0;
        p.pUserData = (&mut *estado) as *mut Estado as *mut c_void;
        p.pfnSequenceCallback = Some(cb_sequencia);
        p.pfnDecodePicture = Some(cb_decodifica);
        p.pfnDisplayPicture = Some(cb_exibe);

        let mut parser: CUvideoparser = std::ptr::null_mut();
        checa("cuvidCreateVideoParser", unsafe {
            (api.create_video_parser)(&mut parser, &mut p)
        })?;

        Ok(Decodificador { parser, estado, _ctx: ctx })
    }

    /// Tamanho do quadro, zero antes do primeiro cabecalho de sequencia.
    pub fn tamanho(&self) -> (u32, u32) {
        (self.estado.w, self.estado.h)
    }

    /// Alimenta bytes do fluxo (Annex-B) e devolve o que ficou pronto.
    ///
    /// Os callbacks rodam DENTRO desta chamada, entao os quadros aparecem
    /// antes de ela retornar -- nao ha nada para esperar depois.
    pub fn alimentar(&mut self, bytes: &[u8]) -> Result<Vec<Nv12>, String> {
        let api = cuvid::Api::get()?;
        let mut pkt: CUVIDSOURCEDATAPACKET = unsafe { std::mem::zeroed() };
        pkt.payload = bytes.as_ptr();
        pkt.payload_size = bytes.len() as _;
        checa("cuvidParseVideoData", unsafe {
            (api.parse_video_data)(self.parser, &mut pkt)
        })?;
        if let Some(e) = self.estado.erro.take() {
            return Err(e);
        }
        Ok(std::mem::take(&mut self.estado.prontos))
    }
}

impl Drop for Decodificador {
    fn drop(&mut self) {
        // Ordem importa: o parser chama os callbacks, que usam o
        // decodificador. Primeiro para de chamar, depois destroi.
        if let Ok(api) = cuvid::Api::get() {
            unsafe {
                if !self.parser.is_null() {
                    let _ = (api.destroy_video_parser)(self.parser);
                }
                if !self.estado.decoder.is_null() {
                    let _ = (api.destroy_decoder)(self.estado.decoder);
                }
            }
        }
    }
}

/// Converte o ponteiro de usuario de volta em `&mut Estado`.
///
/// `unsafe` concentrado aqui: e o unico lugar onde confiamos que o
/// ponteiro que o CUVID devolve e o `Box` que demos a ele.
unsafe fn estado_de(p: *mut c_void) -> Option<&'static mut Estado> {
    (!p.is_null()).then(|| &mut *(p as *mut Estado))
}

/// O fluxo se apresentou: criar o decodificador.
///
/// Devolve o numero de superficies de decodificacao (>0) para seguir, ou
/// 0 para abortar. Zero aqui significa "nao da", e o `parse` devolve erro.
extern "C" fn cb_sequencia(user: *mut c_void, fmt: *mut CUVIDEOFORMAT) -> i32 {
    let Some(st) = (unsafe { estado_de(user) }) else { return 0 };
    let f = unsafe { &*fmt };

    // Area de EXIBICAO, nao a codificada: 1440 vira 1440 coded, mas um
    // 1080 vira 1088 codificado e mostrar isso poria 8 linhas de lixo
    // embaixo.
    let w = (f.display_area.right - f.display_area.left).max(0) as u32;
    let h = (f.display_area.bottom - f.display_area.top).max(0) as u32;
    if w == 0 || h == 0 {
        st.erro = Some("sequencia sem area de exibicao".into());
        return 0;
    }

    let api = match cuvid::Api::get() {
        Ok(a) => a,
        Err(e) => {
            st.erro = Some(e.to_string());
            return 0;
        }
    };

    // Resolucao mudou no meio do fluxo: recria. Acontece de verdade --
    // o host do Windows refaz o encoder quando o monitor muda.
    if !st.decoder.is_null() {
        unsafe { (api.destroy_decoder)(st.decoder) };
        st.decoder = std::ptr::null_mut();
    }

    let mut ci: CUVIDDECODECREATEINFO = unsafe { std::mem::zeroed() };
    ci.ulWidth = f.coded_width as _;
    ci.ulHeight = f.coded_height as _;
    ci.ulMaxWidth = f.coded_width as _;
    ci.ulMaxHeight = f.coded_height as _;
    ci.CodecType = f.codec;
    ci.ChromaFormat = f.chroma_format;
    ci.bitDepthMinus8 = f.bit_depth_luma_minus8 as _;
    ci.OutputFormat = cudaVideoSurfaceFormat_enum::cudaVideoSurfaceFormat_NV12;
    // Weave: o fluxo e progressivo (tela de computador). Bob ou Adaptive
    // existem para video entrelacado e custariam qualidade aqui.
    ci.DeinterlaceMode = cudaVideoDeinterlaceMode_enum::cudaVideoDeinterlaceMode_Weave;
    ci.ulCreationFlags = cudaVideoCreateFlags_enum::cudaVideoCreate_PreferCUVID as _;
    ci.ulNumDecodeSurfaces = f.min_num_decode_surfaces.max(2) as _;
    // Uma superficie de saida: mapeamos e copiamos na hora, dentro do
    // callback. Mais de uma so ajudaria quem segura varios quadros.
    ci.ulNumOutputSurfaces = 1;
    ci.ulTargetWidth = w as _;
    ci.ulTargetHeight = h as _;
    ci.display_area.left = f.display_area.left as i16;
    ci.display_area.top = f.display_area.top as i16;
    ci.display_area.right = f.display_area.right as i16;
    ci.display_area.bottom = f.display_area.bottom as i16;

    let mut dec: CUvideodecoder = std::ptr::null_mut();
    let r = unsafe { (api.create_decoder)(&mut dec, &mut ci) };
    if let Err(e) = checa("cuvidCreateDecoder", r) {
        st.erro = Some(e);
        return 0;
    }
    st.decoder = dec;
    st.w = w;
    st.h = h;
    // Chroma 4:2:0 significa UV com metade das linhas; o resto do codigo
    // assume isso ao copiar `h * 3 / 2` linhas.
    if f.chroma_format != cudaVideoChromaFormat_enum::cudaVideoChromaFormat_420 {
        st.erro = Some(format!("chroma {:?} nao suportado", f.chroma_format));
        return 0;
    }
    ci.ulNumDecodeSurfaces as i32
}

extern "C" fn cb_decodifica(user: *mut c_void, pic: *mut CUVIDPICPARAMS) -> i32 {
    let Some(st) = (unsafe { estado_de(user) }) else { return 0 };
    if st.decoder.is_null() {
        return 0;
    }
    let Ok(api) = cuvid::Api::get() else { return 0 };
    let r = unsafe { (api.decode_picture)(st.decoder, pic) };
    match checa("cuvidDecodePicture", r) {
        Ok(()) => 1,
        Err(e) => {
            st.erro = Some(e);
            0
        }
    }
}

extern "C" fn cb_exibe(user: *mut c_void, info: *mut CUVIDPARSERDISPINFO) -> i32 {
    let Some(st) = (unsafe { estado_de(user) }) else { return 0 };
    if st.decoder.is_null() {
        return 0;
    }
    // `catch_unwind`: desenrolar panico por uma fronteira `extern "C"` e
    // comportamento indefinido, e aqui dentro ha alocacao (que pode
    // falhar). Melhor virar erro nosso que corromper a pilha do driver.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| copia_quadro(st, info)));
    match r {
        Ok(Ok(())) => 1,
        Ok(Err(e)) => {
            st.erro = Some(e);
            0
        }
        Err(_) => {
            st.erro = Some("panico no callback de exibicao".into());
            0
        }
    }
}

fn copia_quadro(st: &mut Estado, info: *mut CUVIDPARSERDISPINFO) -> Result<(), String> {
    let api = cuvid::Api::get()?;
    let d = unsafe { &*info };

    let mut pp: CUVIDPROCPARAMS = unsafe { std::mem::zeroed() };
    pp.progressive_frame = d.progressive_frame;
    pp.top_field_first = d.top_field_first;

    let mut dptr: u64 = 0;
    let mut pitch: u32 = 0;
    checa("cuvidMapVideoFrame", unsafe {
        (api.map_video_frame)(st.decoder, d.picture_index, &mut dptr, &mut pitch, &mut pp)
    })?;

    // Y (h linhas) e UV (h/2 linhas) sao contiguos na superficie mapeada,
    // com o MESMO pitch -- dai uma copia so em vez de duas.
    let linhas = (st.h as usize) * 3 / 2;
    let bytes = pitch as usize * linhas;
    let mut dados = vec![0u8; bytes];
    let copia = unsafe {
        cudarc::driver::sys::cuMemcpyDtoH_v2(
            dados.as_mut_ptr() as *mut c_void,
            dptr as cudarc::driver::sys::CUdeviceptr,
            bytes,
        )
    };
    // Desmapear SEMPRE, inclusive quando a copia falha: superficie
    // mapeada que nao volta trava o decodificador no proximo quadro.
    let _ = unsafe { (api.unmap_video_frame)(st.decoder, dptr) };
    checa("cuMemcpyDtoH", copia)?;

    st.prontos.push(Nv12 { dados, pitch: pitch as usize, w: st.w, h: st.h });
    Ok(())
}

/// NV12 -> BGRA, redimensionando no mesmo passo.
///
/// Uma passada sobre os pixels de SAIDA, nao de entrada: ao encolher
/// (o caso normal, janela menor que a tela remota) isso faz menos
/// trabalho que converter e depois escalar.
///
/// **Amostragem bilinear.** A primeira versao usava vizinho mais
/// proximo, com um comentario dizendo que bilinear era "o proximo passo
/// se incomodar". Incomodou: o usuario comparou Mac->Windows (que
/// decodifica por `swscale`, bilinear) com Ubuntu->Windows (este
/// caminho) e chamou o segundo de pessimo. Encolher texto de 1440p com
/// vizinho mais proximo serrilha cada letra -- e era so no caminho da
/// GPU, o que fazia a diferenca salta aos olhos lado a lado.
///
/// O mapeamento usa centro de pixel (`(x+0.5)*escala-0.5`), nao
/// `x*escala`: sem o meio-pixel a imagem desloca meio pixel para cima e
/// para a esquerda, e em 1:1 a interpolacao deixaria de ser exata.
///
/// BT.709 de faixa limitada: e o que um desktop de 1440p e, e o
/// codificador nao escreve a descricao de cor no fluxo. Conferido contra
/// o ffmpeg no mesmo quadro -- um ponto de diferenca no verde.
pub fn nv12_para_bgra(q: &Nv12, dst: &mut [u32], dw: u32, dh: u32) {
    if dw == 0 || dh == 0 || q.w == 0 || q.h == 0 {
        return;
    }
    let (sw, sh) = (q.w as usize, q.h as usize);
    let plano_uv = q.pitch * sh;
    let ex = sw as f32 / dw as f32;
    let ey = sh as f32 / dh as f32;

    // A coluna de origem nao depende da linha: as mesmas 1451 contas de
    // ponto flutuante estavam sendo refeitas 1232 vezes por quadro. Uma
    // tabela por quadro troca isso por tres leituras de vetor.
    let colunas: Vec<(usize, usize, i32)> = (0..dw as usize)
        .map(|x| {
            let fx = ((x as f32 + 0.5) * ex - 0.5).max(0.0);
            let x0 = (fx as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            (x0, x1, ((fx - x0 as f32) * 256.0) as i32)
        })
        .collect();

    // Uma linha de saida nao olha para nenhuma outra, entao dividir o
    // quadro em faixas e exato, nao aproximado. Sem isso a conversao
    // sozinha custava 28 ms (teto de 35 fps) e segurava o desenho em 27
    // quando a rede ja entregava 60.
    let fatias = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16)
        .min(dh as usize);
    let por_fatia = (dh as usize).div_ceil(fatias);
    let dados = &q.dados;
    let pitch = q.pitch;
    let colunas = &colunas;

    std::thread::scope(|escopo| {
        for (n, faixa) in dst.chunks_mut(por_fatia * dw as usize).enumerate() {
            let y_ini = n * por_fatia;
            escopo.spawn(move || {
                for (dy, linha) in faixa.chunks_mut(dw as usize).enumerate() {
                    let y = y_ini + dy;
                    let fy = ((y as f32 + 0.5) * ey - 0.5).max(0.0);
                    let y0 = (fy as usize).min(sh - 1);
                    let y1 = (y0 + 1).min(sh - 1);
                    let wy = ((fy - y0 as f32) * 256.0) as i32;
                    let l0 = y0 * pitch;
                    let l1 = y1 * pitch;
                    let linha_uv = plano_uv + (y0 / 2) * pitch;

                    for (x, px) in linha.iter_mut().enumerate() {
                        let (x0, x1, wx) = colunas[x];

                        // Dois lerps horizontais e um vertical, em
                        // inteiros. Bilinear so na luma: o olho ve
                        // detalhe em luminancia, e o NV12 ja guarda
                        // croma pela metade -- interpolar os dois
                        // dobraria o laco quente para melhorar o que
                        // ninguem enxerga.
                        let a = dados[l0 + x0] as i32;
                        let b = dados[l0 + x1] as i32;
                        let c = dados[l1 + x0] as i32;
                        let d = dados[l1 + x1] as i32;
                        let topo = a * (256 - wx) + b * wx;
                        let base = c * (256 - wx) + d * wx;
                        let yy = (topo * (256 - wy) + base * wy) >> 16;

                        let i_uv = linha_uv + (x0 & !1);
                        let u = dados[i_uv] as i32;
                        let v = dados[i_uv + 1] as i32;

                        let cc = (yy - 16) * 298;
                        let du = u - 128;
                        let dv = v - 128;
                        let r = (cc + 459 * dv + 128) >> 8;
                        let g = (cc - 55 * du - 136 * dv + 128) >> 8;
                        let bl = (cc + 541 * du + 128) >> 8;
                        *px = ((r.clamp(0, 255) as u32) << 16)
                            | ((g.clamp(0, 255) as u32) << 8)
                            | (bl.clamp(0, 255) as u32);
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod testes {
    /// A conversao tem orcamento de tempo, e ele e testavel.
    ///
    /// Ela roda na CPU para cada quadro desenhado, entao e o teto do
    /// visualizador inteiro: a versao de uma thread so custava 28 ms
    /// (35 fps) e segurava o desenho em 27 enquanto a rede entregava 60.
    /// O usuario viu isso como "qualidade pessima" -- metade era o
    /// serrilhado do vizinho mais proximo, metade era este laco.
    ///
    /// 12 ms e folgado de proposito: medimos 2.8 numa maquina de 12
    /// nucleos, e quem roda os testes pode estar ocupado. O que importa
    /// e nao voltar para a casa dos 28.
    ///
    /// **So em release.** Em `cargo test` sem `--release` o mesmo laco
    /// mede 24 ms e o teste falha sempre -- um alarme falso que custa
    /// tempo de quem for investigar (custou o meu). O que ele vigia e o
    /// codigo otimizado, que e o unico que o usuario roda.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "orcamento de tempo so vale em --release")]
    fn conversao_cabe_no_orcamento() {
        use std::time::Instant;
        let (sw, sh) = (2560usize, 1440usize);
        let mut dados = vec![128u8; sw * sh + sw * sh / 2];
        for (i, b) in dados.iter_mut().enumerate() {
            *b = (i % 255) as u8;
        }
        let q = Nv12 { w: sw as u32, h: sh as u32, pitch: sw, dados };
        let (dw, dh) = (1451u32, 1232u32);
        let mut dst = vec![0u32; (dw * dh) as usize];
        nv12_para_bgra(&q, &mut dst, dw, dh); // aquece cache e paginas

        let t = Instant::now();
        let n = 30;
        for _ in 0..n {
            nv12_para_bgra(&q, &mut dst, dw, dh);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("conversao {sw}x{sh} -> {dw}x{dh}: {ms:.2} ms ({:.0} fps)", 1000.0 / ms);
        assert!(ms < 12.0, "conversao a {ms:.1} ms/quadro: teto de {:.0} fps", 1000.0 / ms);
    }

    use super::*;

    /// Monta um NV12 de cor constante.
    fn solido(w: u32, h: u32, y: u8, u: u8, v: u8) -> Nv12 {
        let pitch = w as usize + 32; // pitch > largura, como o NVDEC faz
        let mut dados = vec![0u8; pitch * (h as usize) * 3 / 2];
        for linha in 0..h as usize {
            for x in 0..w as usize {
                dados[linha * pitch + x] = y;
            }
        }
        let base = pitch * h as usize;
        for linha in 0..(h as usize / 2) {
            for x in (0..w as usize).step_by(2) {
                dados[base + linha * pitch + x] = u;
                dados[base + linha * pitch + x + 1] = v;
            }
        }
        Nv12 { dados, pitch, w, h }
    }

    fn canais(px: u32) -> (u8, u8, u8) {
        (((px >> 16) & 0xff) as u8, ((px >> 8) & 0xff) as u8, (px & 0xff) as u8)
    }

    #[test]
    fn preto_e_branco_chegam_nos_extremos() {
        // Faixa limitada: 16 e preto, 235 e branco. Tratar como faixa
        // cheia daria cinza em vez de preto, que e o erro classico.
        let mut dst = vec![0u32; 4];
        nv12_para_bgra(&solido(2, 2, 16, 128, 128), &mut dst, 2, 2);
        assert_eq!(canais(dst[0]), (0, 0, 0), "16 tem de virar preto");

        nv12_para_bgra(&solido(2, 2, 235, 128, 128), &mut dst, 2, 2);
        let (r, g, b) = canais(dst[0]);
        assert!(r > 250 && g > 250 && b > 250, "235 virou {r},{g},{b}");
    }

    #[test]
    fn cinza_medio_fica_cinza() {
        let mut dst = vec![0u32; 4];
        nv12_para_bgra(&solido(2, 2, 126, 128, 128), &mut dst, 2, 2);
        let (r, g, b) = canais(dst[0]);
        assert!(r == g && g == b, "cinza saiu colorido: {r},{g},{b}");
        assert!((100..=140).contains(&r), "cinza medio virou {r}");
    }

    #[test]
    fn respeita_o_pitch_e_nao_le_lixo() {
        // O pitch e maior que a largura: ler linha a linha por `w` em vez
        // de por `pitch` embaralharia a imagem progressivamente -- um
        // erro que aparece como inclinacao, nao como falha.
        let w = 8;
        let h = 4;
        let mut q = solido(w, h, 16, 128, 128);
        // Marca o primeiro pixel de cada linha com branco.
        for linha in 0..h as usize {
            q.dados[linha * q.pitch] = 235;
        }
        let mut dst = vec![0u32; (w * h) as usize];
        nv12_para_bgra(&q, &mut dst, w, h);
        for linha in 0..h as usize {
            let (r, _, _) = canais(dst[linha * w as usize]);
            assert!(r > 250, "linha {linha} perdeu a marca -- pitch ignorado");
            let (r2, _, _) = canais(dst[linha * w as usize + 1]);
            assert!(r2 < 5, "linha {linha} vazou a marca para o vizinho");
        }
    }

    #[test]
    fn encolher_nao_estoura_o_destino() {
        // Destino menor que a origem: o laco anda pelo DESTINO, entao
        // nenhuma escrita pode passar do fim.
        let q = solido(64, 64, 100, 128, 128);
        let mut dst = vec![0u32; 7 * 5];
        nv12_para_bgra(&q, &mut dst, 7, 5);
        assert!(dst.iter().all(|p| *p != 0), "sobrou pixel nao escrito");
    }

    #[test]
    fn tamanho_zero_nao_faz_nada() {
        let q = solido(4, 4, 100, 128, 128);
        let mut dst = vec![0u32; 1];
        nv12_para_bgra(&q, &mut dst, 0, 0);
        assert_eq!(dst[0], 0);
    }
}
