//! Captura no Windows: DXGI Desktop Duplication.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Por que DXGI e nao GDI
//!
//! `BitBlt` do GDI copia a tela pela CPU, nao ve janelas aceleradas e
//! custa caro em 1440p. O Desktop Duplication entrega a textura que o
//! compositor **ja montou**, na GPU, e avisa quando nada mudou -- o que
//! significa zero trabalho com a tela parada. E o mesmo caminho que o
//! Parsec e o OBS usam.
//!
//! ## O cursor nao vem na imagem
//!
//! Esta e a diferenca visivel em relacao ao host do Linux, onde pedimos
//! ao Mutter `cursor-mode: 1` e ele embute o cursor no quadro. O DXGI
//! entrega a tela **sem** cursor e o ponteiro em separado
//! (`GetFramePointerShape`), porque e assim que o hardware compoe.
//!
//! Nao desenhamos o cursor por enquanto, e isso e menos grave do que
//! parece: o cliente manda posicao ABSOLUTA, entao o cursor da sua
//! propria maquina fica exatamente sobre o ponto remoto -- com zero
//! latencia, inclusive. O que falta e a FORMA (a barra de texto, as
//! setas de redimensionar). O Parsec faz igual por este motivo.
//!
//! ## A copia para a CPU e consciente
//!
//! O certo seria registrar a textura D3D11 direto no NVENC
//! (`NV_ENC_DEVICE_TYPE_DIRECTX`) e nunca sair da GPU. Fazemos a volta
//! pela CPU -- textura de staging, `Map`, upload para CUDA -- por um
//! motivo: e EXATAMENTE o caminho que o host do Linux ja faz (o PipeWire
//! entrega em memoria de CPU), entao o codificador e um so. Trocar isto
//! e otimizacao localizada aqui, nao mudanca de arquitetura.

#![cfg(windows)]

use crate::cursor::{desenhar, recorte, Forma, TipoForma};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ_WRITE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
};

/// O que saiu de uma tentativa de pegar quadro.
#[derive(Debug, PartialEq, Eq)]
pub enum Estado {
    /// Imagem nova na textura de staging; da para mapear e codificar.
    Novo,
    /// Nada mudou dentro do prazo. Tela parada e o caso comum, nao erro.
    Parado,
    /// A duplicacao morreu -- mudanca de resolucao, tela de UAC, jogo em
    /// tela cheia assumindo o display. Tem de recriar.
    Perdido,
}

/// Pixels mapeados da textura de staging. Desmapeia ao sair de escopo.
///
/// Guard e nao `(ponteiro, tamanho)` porque esquecer o `Unmap` trava a
/// textura para sempre e o proximo `CopyResource` falha calado.
pub struct Mapeado<'a> {
    contexto: &'a ID3D11DeviceContext,
    recurso: ID3D11Resource,
    pub px: &'a [u8],
    pub stride: usize,
}

impl Drop for Mapeado<'_> {
    fn drop(&mut self) {
        unsafe { self.contexto.Unmap(&self.recurso, 0) };
    }
}

/// O monitor, antes de a duplicacao comecar.
///
/// Separado de `Tela` por uma razao medida: segurar a duplicacao do DXGI
/// enquanto o CUDA inicializa **travava o processo**. O log parava em
/// "DXGI: monitor 0, 2560x1440" com 0,2 s de CPU e ficava ali -- a mesma
/// sonda de NVENC, sozinha, rodava a 111 fps na mesma sessao. Entao a
/// ordem passou a ser: abrir o monitor, ler o tamanho, montar o encoder,
/// e SO DEPOIS duplicar.
///
/// De quebra a ordem nova e melhor desenho: se o NVENC nao estiver
/// disponivel, falhamos antes de tomar a tela de alguem.
pub struct Monitor {
    dispositivo: ID3D11Device,
    contexto: ID3D11DeviceContext,
    saida: IDXGIOutput1,
    w: u32,
    h: u32,
    /// Canto do monitor no desktop virtual. O input precisa dele para
    /// converter pixel do stream em posicao absoluta da maquina.
    x: i32,
    y: i32,
}

impl Monitor {
    /// Abre o monitor `indice` (0 = primario) sem comecar a capturar.
    pub fn abrir(indice: u32) -> anyhow::Result<Self> {
        unsafe {
            let mut dispositivo: Option<ID3D11Device> = None;
            let mut contexto: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                // BGRA_SUPPORT: o desktop e BGRA, e sem a flag a criacao
                // da textura de staging pode ser recusada no formato.
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut dispositivo),
                None,
                Some(&mut contexto),
            )?;
            let dispositivo = dispositivo
                .ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice nao devolveu dispositivo"))?;
            let contexto = contexto
                .ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice nao devolveu contexto"))?;

            // Do dispositivo para o adaptador, do adaptador para a saida:
            // e preciso duplicar a saida do MESMO adaptador que criou o
            // dispositivo, senao a textura atravessaria placas.
            let dxgi: IDXGIDevice = dispositivo.cast()?;
            let adaptador = dxgi.GetAdapter()?;
            let saida = adaptador
                .EnumOutputs(indice)
                .map_err(|e| anyhow::anyhow!("monitor {indice}: {e}"))?;
            let saida: IDXGIOutput1 = saida.cast()?;
            let desc = saida.GetDesc()?;
            let r = desc.DesktopCoordinates;
            let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
            println!("  DXGI: monitor {indice}, {w}x{h}");
            Ok(Monitor { dispositivo, contexto, saida, w, h, x: r.left, y: r.top })
        }
    }

    pub fn tamanho(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Canto superior esquerdo do monitor dentro do desktop virtual.
    pub fn origem(&self) -> (i32, i32) {
        (self.x, self.y)
    }

    /// Comeca a capturar. A partir daqui o compositor esta nos esperando
    /// a cada quadro, entao tudo que e lento tem de estar pronto.
    ///
    /// Tenta algumas vezes antes de desistir: `DuplicateOutput` recusa
    /// por motivos TEMPORARIOS (tela de UAC na frente, sessao trocando)
    /// e morrer na primeira faria o botao "Ver a tela" falhar por causa
    /// de um dialogo que sai sozinho em dois segundos.
    pub fn duplicar(self) -> anyhow::Result<Tela> {
        const TENTATIVAS: u32 = 5;
        let mut ultimo = String::new();
        for i in 0..TENTATIVAS {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(600));
            }
            match unsafe { self.saida.DuplicateOutput(&self.dispositivo) } {
                Ok(dup) => {
                    let staging = cria_staging(&self.dispositivo, self.w, self.h)?;
                    return Ok(Tela {
                        m: self,
                        dup,
                        staging,
                        tem_imagem: false,
                        forma: None,
                        ponteiro: (0, 0),
                        visivel: false,
                        salvo: None,
                        desenhou_uma_vez: false,
                        visivel_sistema: false,
                    });
                }
                Err(e) => {
                    ultimo = e.to_string();
                    println!("  DuplicateOutput recusou ({}/{TENTATIVAS}): {e}", i + 1);
                }
            }
        }
        // "Acesso negado" sozinho manda a pessoa procurar permissao de
        // usuario, que NUNCA e o problema. Sao quatro causas com o
        // mesmo codigo, e ja diagnostiquei a errada tres vezes num dia
        // -- por isso a ordem aqui e por frequencia MEDIDA, nao por
        // intuicao:
        //
        // 1. Protetor de tela rodando. Ele vive num desktop proprio e o
        //    DXGI nao duplica para quem nao esta no desktop de input.
        //    Foi a causa do "nao consigo acordar o Windows", com
        //    LogonUI ausente e adaptador de video OK -- os dois sinais
        //    que me fizeram descartar as outras hipoteses. O host ja
        //    tenta encerra-lo na partida.
        // 2. Tela bloqueada de verdade (LogonUI na frente).
        // 3. Dialogo de UAC no desktop seguro.
        // 4. Pilha de display meio instalada -- foi real quando o
        //    driver virtual do Parsec saiu e deixou um `Microsoft Basic
        //    Display Adapter` em estado `Unknown` com reinicio
        //    pendente.
        //
        // O roteiro para distinguir esta em
        // .claude/rules/windows-locked-screen.md.
        anyhow::bail!(
            "DuplicateOutput recusou {TENTATIVAS} vezes ({ultimo}). \
             Quatro causas dao este mesmo erro; confira nesta ordem: \
             (1) protetor de tela rodando -- `Get-Process | ? Path -like '*.scr'`; \
             (2) tela bloqueada -- `Get-Process LogonUI`; \
             (3) dialogo de UAC na frente -- `Get-Process consent`; \
             (4) driver de video meio instalado -- `Get-CimInstance Win32_VideoController`. \
             O DXGI nao captura desktop que nao seja o de input, por desenho."
        )
    }
}

pub struct Tela {
    m: Monitor,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    /// Houve ao menos um quadro: sem isto, mapear leria lixo.
    tem_imagem: bool,
    /// Ultimo desenho de ponteiro que o DXGI mandou. Ele so reenvia
    /// quando a forma MUDA, entao guardar e obrigatorio -- sem isto o
    /// cursor apareceria num quadro e sumiria no seguinte.
    forma: Option<Forma>,
    ponteiro: (i32, i32),
    visivel: bool,
    /// O que estava sob o cursor antes de desenharmos, e onde.
    ///
    /// Compomos o cursor DENTRO da textura de staging, que nao e
    /// recopiada quando so o ponteiro se move. Sem desfazer o desenho
    /// anterior, cada movimento deixaria um cursor grudado para tras --
    /// um rastro de setas pela tela.
    salvo: Option<(u32, u32, u32, u32, Vec<u8>)>,
    /// So para o log dizer uma vez que a composicao esta de pe.
    desenhou_uma_vez: bool,
    /// O que o DXGI diz sobre o cursor DO SISTEMA estar visivel.
    visivel_sistema: bool,
}

impl Tela {
    pub fn tamanho(&self) -> (u32, u32) {
        self.m.tamanho()
    }

    pub fn origem(&self) -> (i32, i32) {
        self.m.origem()
    }

    pub fn tem_imagem(&self) -> bool {
        self.tem_imagem
    }

    /// Tenta pegar um quadro novo, esperando no maximo `espera_ms`.
    ///
    /// Em caso de `Novo`, a imagem fica na textura de staging e sai por
    /// `mapear`. A separacao existe para o `ReleaseFrame` acontecer
    /// **antes** do `Map`: enquanto o quadro nao e devolvido, o
    /// compositor espera por nos.
    pub fn proximo(&mut self, espera_ms: u32) -> anyhow::Result<Estado> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut recurso: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(espera_ms, &mut info, &mut recurso) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Estado::Parado),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => return Ok(Estado::Perdido),
                Err(e) => return Err(anyhow::anyhow!("AcquireNextFrame: {e}")),
            }

            // Diagnostico cru dos primeiros quadros: e o unico jeito
            // de saber o que o DXGI realmente reporta sobre o ponteiro.
            // Posicao pelo `GetCursorPos`, nao pelo DXGI.
            //
            // O DXGI so reporta ponteiro quando existe um sendo
            // DESENHADO. Nesta maquina nao existe: ela nao tem mouse
            // fisico -- o dongle fica do outro lado --, e o Windows
            // esconde o cursor quando nao ha dispositivo apontador.
            // Medido com `GetCursorInfo`: flags=0, hCursor=0, e ainda
            // assim a posicao correta. Por isso o hover respondia e
            // nada aparecia.
            //
            // `GetCursorPos` devolve a posicao de qualquer jeito,
            // visivel ou nao, que e exatamente o que precisamos.
            let mut pt = POINT::default();
            let mexeu = if GetCursorPos(&mut pt).is_ok() {
                let novo = (pt.x, pt.y);
                let mudou = novo != self.ponteiro;
                self.ponteiro = novo;
                mudou
            } else {
                false
            };
            // Visibilidade: so acreditar no DXGI quando ele tem um
            // cursor de verdade para falar sobre.
            //
            // Sem mouse na maquina, `Visible` vem false para sempre e
            // obedecer isso apagaria a seta embutida -- que existe
            // justamente para esse caso. Com mouse, `Visible` passa a
            // significar algo: jogo em tela cheia esconde o ponteiro de
            // proposito, e desenhar um por cima seria nos intrometendo.
            if info.LastMouseUpdateTime != 0 {
                self.visivel_sistema = info.PointerPosition.Visible.as_bool();
            }
            self.visivel = self.forma.is_none() || self.visivel_sistema;

            // Se o sistema TEM um cursor, ele manda a forma quando ela
            // muda, e desenhar a dele e melhor que a nossa (mao, barra
            // de texto, redimensionar). Quando nunca vem nada, usamos a
            // seta embutida -- ver `cursor::seta_embutida`.
            if info.PointerShapeBufferSize > 0 {
                self.ler_forma(info.PointerShapeBufferSize);
            }

            // `LastPresentTime == 0` quer dizer que so o ponteiro se
            // moveu -- a imagem do desktop e a mesma.
            let so_ponteiro = info.LastPresentTime == 0;

            // Mas "so o ponteiro" deixou de ser motivo para nao mandar
            // nada: desde que desenhamos o cursor no quadro, mover o
            // mouse MUDA a imagem. Num KVM isso e o principal: a pessoa
            // olha a tela remota para saber onde o cursor esta. O
            // re-encode e barato -- a cena e identica fora do cursor, e
            // sai um P-frame de poucos bytes.
            let estado = if !so_ponteiro || (mexeu && self.tem_imagem) {
                Estado::Novo
            } else {
                Estado::Parado
            };

            if !so_ponteiro {
                if let Some(r) = &recurso {
                    let tex: ID3D11Texture2D = r.cast()?;
                    self.m.contexto.CopyResource(&self.staging, &tex);
                    self.tem_imagem = true;
                    // A copia nova apagou o cursor que tinhamos
                    // desenhado, entao nao ha o que desfazer.
                    self.salvo = None;
                }
            }
            // Sempre: devolver o quadro e obrigacao, mesmo quando so o
            // ponteiro mudou. Nao devolver trava a captura no proximo
            // Acquire com "ja existe quadro em aberto".
            let _ = self.dup.ReleaseFrame();
            Ok(estado)
        }
    }

    /// Le o desenho do ponteiro que o DXGI acabou de anunciar.
    fn ler_forma(&mut self, tamanho: u32) {
        let mut buf = vec![0u8; tamanho as usize];
        let mut usado = 0u32;
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        let r = unsafe {
            self.dup.GetFramePointerShape(
                tamanho,
                buf.as_mut_ptr() as *mut _,
                &mut usado,
                &mut info,
            )
        };
        if r.is_err() {
            return;
        }
        let tipo = match info.Type {
            1 => TipoForma::Mono,
            2 => TipoForma::Cor,
            4 => TipoForma::CorMascarada,
            // Tipo que nao conhecemos: melhor NENHUM cursor que um
            // retangulo de lixo no meio da tela de alguem.
            outro => {
                eprintln!("  forma de ponteiro desconhecida: {outro}");
                self.forma = None;
                return;
            }
        };
        buf.truncate(usado as usize);
        println!(
            "  forma de ponteiro: {:?} {}x{} pitch={} ({} bytes)",
            tipo, info.Width, info.Height, info.Pitch, usado
        );
        self.forma = Some(Forma {
            tipo,
            w: info.Width,
            h: info.Height,
            pitch: info.Pitch as usize,
            bytes: buf,
            hot: (info.HotSpot.x, info.HotSpot.y),
        });
    }

    /// Mapeia a ultima imagem copiada para leitura pela CPU.
    ///
    /// Serve tanto para o quadro que acabou de chegar quanto para
    /// reenviar o ultimo com a tela parada -- o staging guarda a imagem,
    /// entao reenviar nao custa copia nova.
    pub fn mapear(&mut self) -> anyhow::Result<Mapeado<'_>> {
        unsafe {
            let recurso: ID3D11Resource = self.staging.cast()?;
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.m.contexto.Map(&recurso, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut m))?;
            let stride = m.RowPitch as usize;
            let (w, h) = (self.m.w, self.m.h);
            let total = stride * h as usize;
            let mut_px = std::slice::from_raw_parts_mut(m.pData as *mut u8, total);

            // 1. Desfazer o cursor anterior. So existe quando nenhuma
            //    copia nova passou por cima dele.
            if let Some((rx, ry, rw, rh, dados)) = self.salvo.take() {
                for ly in 0..rh as usize {
                    let o = (ry as usize + ly) * stride + rx as usize * 4;
                    let largura = rw as usize * 4;
                    let fonte = ly * largura;
                    if o + largura <= total && fonte + largura <= dados.len() {
                        mut_px[o..o + largura].copy_from_slice(&dados[fonte..fonte + largura]);
                    }
                }
            }

            // 2. Guardar o que esta sob o cursor novo e desenhar.
            if self.visivel {
                let embutida;
                let forma = match &self.forma {
                    Some(f) => f,
                    None => {
                        embutida = crate::cursor::seta_embutida();
                        &embutida
                    }
                };
                {
                    let f = forma;
                    let alt = if f.tipo == TipoForma::Mono { f.h / 2 } else { f.h };
                    // Descontar o hotspot aqui esta CERTO porque a
                    // posicao vem do `GetCursorPos`, que devolve o
                    // ponto quente (a pontinha da seta).
                    //
                    // Nao vale para a posicao do DXGI: a documentacao
                    // do `DXGI_OUTDUPL_POINTER_SHAPE_INFO` diz que o
                    // app deve por o canto superior esquerdo em
                    // `PointerPosition.Position` e **nao** usar o
                    // hotspot ali. Quem trocar a fonte da posicao tem
                    // de tirar estas duas linhas junto, senao o cursor
                    // desenha deslocado pelo tamanho do hotspot.
                    let x = self.ponteiro.0 - f.hot.0;
                    let y = self.ponteiro.1 - f.hot.1;
                    if let Some((rx, ry, rw, rh)) = recorte(w, h, f.w, alt, x, y) {
                        let largura = rw as usize * 4;
                        let mut dados = Vec::with_capacity(largura * rh as usize);
                        for ly in 0..rh as usize {
                            let o = (ry as usize + ly) * stride + rx as usize * 4;
                            if o + largura <= total {
                                dados.extend_from_slice(&mut_px[o..o + largura]);
                            }
                        }
                        self.salvo = Some((rx, ry, rw, rh, dados));
                        desenhar(mut_px, stride, w, h, f, x, y);
                        if !self.desenhou_uma_vez {
                            self.desenhou_uma_vez = true;
                            println!("  cursor desenhado em {x},{y} ({rw}x{rh})");
                        }
                    }
                }
            }

            let px = std::slice::from_raw_parts(m.pData as *const u8, total);
            Ok(Mapeado { contexto: &self.m.contexto, recurso, px, stride })
        }
    }

    /// Recria a duplicacao depois de `Perdido`.
    ///
    /// A resolucao pode ter mudado junto (foi o que derrubou a anterior),
    /// entao a textura de staging e o tamanho sao refeitos tambem --
    /// devolve `true` quando quem chama precisa refazer o codificador.
    pub fn recriar(&mut self) -> anyhow::Result<bool> {
        unsafe {
            let desc = self.m.saida.GetDesc()?;
            let r = desc.DesktopCoordinates;
            let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
            self.dup = self
                .m
                .saida
                .DuplicateOutput(&self.m.dispositivo)
                .map_err(|e| anyhow::anyhow!("DuplicateOutput: {e}"))?;
            // A origem pode mudar sem o tamanho mudar (monitor arrastado
            // para o outro lado no painel de controle), entao ela e
            // atualizada fora do `if`.
            self.m.x = r.left;
            self.m.y = r.top;
            let mudou = (w, h) != (self.m.w, self.m.h);
            if mudou {
                self.staging = cria_staging(&self.m.dispositivo, w, h)?;
                self.m.w = w;
                self.m.h = h;
                self.tem_imagem = false;
                println!("  DXGI: resolucao mudou para {w}x{h}");
            }
            Ok(mudou)
        }
    }
}

/// Textura que a CPU consegue ler. `STAGING` + `CPU_ACCESS_READ` e a
/// unica combinacao que o D3D11 deixa mapear para leitura.
fn cria_staging(d: &ID3D11Device, w: u32, h: u32) -> anyhow::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: (D3D11_CPU_ACCESS_READ.0 | D3D11_CPU_ACCESS_WRITE.0) as u32,
        MiscFlags: 0,
    };
    let mut t: Option<ID3D11Texture2D> = None;
    unsafe { d.CreateTexture2D(&desc, None, Some(&mut t))? };
    t.ok_or_else(|| anyhow::anyhow!("CreateTexture2D nao devolveu textura"))
}
