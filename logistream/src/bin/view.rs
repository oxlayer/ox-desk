//! Visualizador: QUIC -> H.264 -> janela. Fecha a v2.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Onde cada coisa roda
//!
//! Rede e decodificacao numa thread, desenho na thread da interface. O
//! laco de janela e dono da thread principal (exigencia do macOS, boa
//! pratica no resto) e nao pode bloquear -- se decodificasse ali, cada
//! quadro travaria a interface.
//!
//! ## Onde ele compila
//!
//! Nas tres. Quem decodifica muda por plataforma e esta em
//! `logistream::decode`: NVDEC no Linux e no Windows (vem com o driver
//! NVIDIA), `ffmpeg` no Mac (que nao tem NVIDIA). Foi o NVDEC que tirou
//! o Windows do RDP -- la nao existe `libavcodec` e levar as DLLs para
//! la seria encanamento sem ganho.
//!
//! Entre as duas ha **um slot**, nao uma fila: quadro novo sobrescreve o
//! que ainda nao foi desenhado. Fila so acumularia atraso, e numa tela
//! remota quadro velho nao tem valor -- ja passou.
//!
//!   cargo run --release --bin view -- <host:porta>

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn main() {
    eprintln!(
        "o visualizador nao compila neste sistema: ele precisa de NVDEC \
         (Linux/Windows com GPU NVIDIA) ou do libavcodec (macOS). \
         Ver specs/010."
    );
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn main() -> anyhow::Result<()> {
    visual::run()
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
mod visual {
    use std::num::NonZeroU32;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use logistream::input::{evdev_de_winit, Evento, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
    use logistream::net::{self, Reassembler};
    use winit::event::{ElementState, MouseButton, MouseScrollDelta};
    use winit::keyboard::PhysicalKey;
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::window::{Window, WindowId};

    /// O quadro pronto para desenhar, com a geometria que o explica.
    ///
    /// Tres tamanhos diferentes vivem aqui, e confundi-los foi um bug
    /// visivel: a tela REMOTA (`rw`x`rh`), a IMAGEM desenhada (`w`x`h`,
    /// a remota ajustada a janela sem distorcer) e a JANELA (`jw`x`jh`).
    ///
    /// O calculo da posicao do mouse usava o tamanho da imagem como se
    /// fosse o da tela remota. Como a imagem tem o tamanho da janela,
    /// `posicao / janela * imagem` devolvia a propria posicao -- numa
    /// janela de 1280x720 contra uma tela de 2560x1440, o cursor so
    /// alcancava o QUARTO superior esquerdo do desktop remoto. Era o que
    /// o usuario via como "a posicao nao bate".
    #[derive(Default)]
    pub struct Slot {
        pub rgb: Vec<u32>,
        /// Tamanho da imagem desenhada.
        pub w: u32,
        pub h: u32,
        /// Canto onde a imagem comeca dentro da janela (barras laterais).
        pub ox: u32,
        pub oy: u32,
        /// Tamanho da tela remota -- o espaco de coordenadas do host.
        pub rw: u32,
        pub rh: u32,
        /// Janela para a qual esta geometria foi calculada.
        pub jw: u32,
        pub jh: u32,
        /// Sobe a cada quadro novo; a janela so redesenha quando muda.
        pub geracao: u64,
        /// Instante da captura do quadro no slot, para `LOGI_MEDIR`.
        pub capturado_ns: u64,
    }

    /// Cor das barras quando a proporcao da janela nao bate com a da tela
    /// remota. Escura e neutra: barra clara puxa o olho para fora da
    /// imagem.
    const FUNDO: u32 = 0x0010_1014;

    /// Imagem ajustada a janela SEM distorcer: escala pelo lado que
    /// aperta, centraliza o resto.
    ///
    /// Antes a imagem era esticada para o tamanho exato da janela. Numa
    /// tela 16:10 mostrando um desktop 16:9 isso deforma tudo -- e, pior,
    /// um circulo no host nao e um circulo aqui, o que atrapalha
    /// justamente quem esta tentando trabalhar na outra maquina.
    fn encaixa(rw: u32, rh: u32, jw: u32, jh: u32) -> (u32, u32, u32, u32) {
        if rw == 0 || rh == 0 || jw == 0 || jh == 0 {
            return (0, 0, 0, 0);
        }
        let e = (jw as f64 / rw as f64).min(jh as f64 / rh as f64);
        let w = ((rw as f64 * e).round() as u32).clamp(1, jw);
        let h = ((rh as f64 * e).round() as u32).clamp(1, jh);
        ((jw - w) / 2, (jh - h) / 2, w, h)
    }

    /// Posicao na janela -> posicao na tela remota.
    ///
    /// `None` quando o cursor esta sobre a barra e nao sobre a imagem:
    /// nao ha ponto remoto correspondente, e grudar na beirada inventaria
    /// um movimento que a pessoa nao fez.
    ///
    /// Pura e separada do laco de eventos porque foi exatamente aqui que
    /// o bug morou -- e um erro de escala nao da erro, so poe o cursor
    /// no lugar errado, o que se descobre usando.
    #[allow(clippy::type_complexity)]
    fn ponto_remoto(
        px: f64,
        py: f64,
        (ox, oy, iw, ih, rw, rh): (f64, f64, f64, f64, f64, f64),
    ) -> Option<(f64, f64)> {
        if iw <= 0.0 || ih <= 0.0 {
            return None;
        }
        let x = (px - ox) / iw * rw;
        let y = (py - oy) / ih * rh;
        if x < 0.0 || y < 0.0 || x >= rw || y >= rh {
            return None;
        }
        Some((x, y))
    }

    #[cfg(test)]
    mod testes {
        use super::*;

        #[test]
        fn mesma_proporcao_preenche_sem_barra() {
            // 2560x1440 numa janela 1280x720: cabe exato, sem barra.
            assert_eq!(encaixa(2560, 1440, 1280, 720), (0, 0, 1280, 720));
        }

        #[test]
        fn janela_mais_larga_ganha_barra_lateral() {
            // 16:9 numa janela 2:1 -> aperta pela altura, sobra largura.
            // 1920 * (1000/1080) = 1777,8 -> 1778, arredondado e nao
            // truncado: truncar encolhe a imagem um pixel a cada
            // redimensionamento e a conta do cursor herda o erro.
            let (ox, oy, w, h) = encaixa(1920, 1080, 2000, 1000);
            assert_eq!((w, h), (1778, 1000));
            assert_eq!(oy, 0, "sem barra em cima quando aperta pela altura");
            assert_eq!(ox, (2000 - 1778) / 2, "centralizada");
        }

        #[test]
        fn janela_mais_alta_ganha_barra_acima() {
            let (ox, oy, w, h) = encaixa(1920, 1080, 1000, 1000);
            assert_eq!((w, h), (1000, 563));
            assert_eq!(ox, 0);
            assert_eq!(oy, (1000 - 563) / 2);
        }

        #[test]
        fn a_imagem_nunca_passa_da_janela() {
            // Arredondar para cima nao pode criar imagem maior que a
            // janela: o desenho recortaria e a conta do cursor mentiria.
            for (rw, rh, jw, jh) in
                [(2560u32, 1440u32, 1281u32, 721u32), (1366, 768, 999, 1), (3, 7, 100, 100)]
            {
                let (ox, oy, w, h) = encaixa(rw, rh, jw, jh);
                assert!(w <= jw && h <= jh, "{rw}x{rh} em {jw}x{jh} deu {w}x{h}");
                assert!(ox + w <= jw && oy + h <= jh);
            }
        }

        #[test]
        fn tamanho_zero_nao_divide_por_zero() {
            assert_eq!(encaixa(0, 0, 100, 100), (0, 0, 0, 0));
            assert_eq!(encaixa(100, 100, 0, 0), (0, 0, 0, 0));
            assert_eq!(ponto_remoto(1.0, 1.0, (0.0, 0.0, 0.0, 0.0, 10.0, 10.0)), None);
        }

        #[test]
        fn o_cursor_cobre_a_tela_remota_inteira() {
            // O bug: numa janela de 1280x720 contra 2560x1440, o cursor
            // so alcancava o quarto superior esquerdo. O canto inferior
            // direito da janela tem de chegar ao canto da tela remota.
            let g = (0.0, 0.0, 1280.0, 720.0, 2560.0, 1440.0);
            assert_eq!(ponto_remoto(0.0, 0.0, g), Some((0.0, 0.0)));
            assert_eq!(ponto_remoto(640.0, 360.0, g), Some((1280.0, 720.0)));
            let (x, y) = ponto_remoto(1279.0, 719.0, g).expect("dentro");
            assert!(x > 2550.0 && y > 1430.0, "canto virou {x},{y}");
        }

        #[test]
        fn sobre_a_barra_nao_manda_nada() {
            // Imagem de 800x600 centralizada numa janela de 1000x600:
            // barra de 100px em cada lado.
            let g = (100.0, 0.0, 800.0, 600.0, 1920.0, 1440.0);
            assert_eq!(ponto_remoto(50.0, 300.0, g), None, "barra esquerda");
            assert_eq!(ponto_remoto(950.0, 300.0, g), None, "barra direita");
            assert!(ponto_remoto(100.0, 300.0, g).is_some(), "primeira coluna da imagem");
            // E o inicio da imagem e o inicio da tela remota, nao um
            // deslocamento de 100px.
            assert_eq!(ponto_remoto(100.0, 0.0, g), Some((0.0, 0.0)));
        }
    }

    pub fn run() -> anyhow::Result<()> {
        let alvo = std::env::args().nth(1).unwrap_or_else(|| "ubuntu-host.local:9777".into());
        let slot: Arc<Mutex<Slot>> = Arc::new(Mutex::new(Slot::default()));
        let slot_rede = Arc::clone(&slot);

        // Canal do tokio usado de codigo SINCRONO (o laco de janela):
        // `try_send` nao bloqueia, e se encher e porque o input esta
        // chegando mais rapido que a rede -- descartar movimento de
        // mouse e aceitavel, e os eventos de tecla sao raros o bastante
        // para nunca encherem isto.
        let (tx_in, rx_in) = tokio::sync::mpsc::channel::<Evento>(256);

        // O tamanho da JANELA, compartilhado com quem escala.
        //
        // Eu dimensionava a superficie pelo tamanho do QUADRO: numa
        // janela de 1280x720 mostrando um stream de 2560x1440, o buffer
        // nao casa com a janela e o resultado e indefinido -- foi o que
        // o usuario viu como "imagem bugada". Quem redimensiona agora e
        // o `swscale`, que ja estava no caminho de qualquer forma.
        let tam: Arc<Mutex<(u32, u32)>> = Arc::new(Mutex::new((1280, 720)));
        let tam_rede = Arc::clone(&tam);

        // O laco da janela tem de ser criado ANTES da thread de rede: e
        // dele que sai o `proxy`, e e por ele que a rede acorda a
        // interface quando chega quadro.
        let ev = EventLoop::new()?;
        let proxy = ev.create_proxy();

        std::thread::spawn(move || {
            if let Err(e) = receber(&alvo, slot_rede, rx_in, tam_rede, proxy) {
                eprintln!("rede: {e}");
                std::process::exit(1);
            }
        });

        // `Wait` e nao `Poll`: acordamos quando chega quadro, nao a
        // 100% de CPU perguntando se chegou.
        ev.set_control_flow(ControlFlow::Wait);
        let mut app = App {
            janela: None,
            surface: None,
            slot,
            desenhada: 0,
            n: 0,
            t0: Instant::now(),
            tx_in,
            tam_janela: (1280.0, 720.0),
            tam,
            medir: std::env::var_os("LOGI_MEDIR").is_some(),
            atrasos: Vec::new(),
        };
        ev.run_app(&mut app)?;
        Ok(())
    }

    struct App {
        janela: Option<Arc<Window>>,
        surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
        slot: Arc<Mutex<Slot>>,
        desenhada: u64,
        n: u64,
        t0: Instant,
        tx_in: tokio::sync::mpsc::Sender<Evento>,
        /// Tamanho atual da janela, para converter a posicao do cursor
        /// no espaco do stream.
        tam_janela: (f64, f64),
        /// Mesmo tamanho, em pixels fisicos, para a thread que escala.
        tam: Arc<Mutex<(u32, u32)>>,
        /// `LOGI_MEDIR=1`: imprime o atraso da captura ate a tela.
        medir: bool,
        atrasos: Vec<f64>,
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, ev: &ActiveEventLoop) {
            let atrib = Window::default_attributes()
                .with_title("logistream")
                .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
            let janela = Arc::new(ev.create_window(atrib).expect("janela"));
            let ctx = softbuffer::Context::new(janela.clone()).expect("contexto");
            let surface = softbuffer::Surface::new(&ctx, janela.clone()).expect("surface");
            self.janela = Some(janela);
            self.surface = Some(surface);
        }

        fn window_event(&mut self, ev: &ActiveEventLoop, _: WindowId, evento: WindowEvent) {
            match evento {
                WindowEvent::CloseRequested => ev.exit(),
                WindowEvent::RedrawRequested => self.desenhar(),
                WindowEvent::Resized(t) => {
                    self.tam_janela = (t.width as f64, t.height as f64);
                    if let Ok(mut g) = self.tam.lock() {
                        *g = (t.width.max(1), t.height.max(1));
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    // A posicao vai em coordenadas da TELA REMOTA -- e o
                    // host que define o espaco, nao nos. E a conversao e
                    // pela IMAGEM desenhada, nao pela janela: com barras
                    // laterais as duas diferem, e usar a janela jogaria o
                    // cursor remoto para o lugar errado por todo o
                    // tamanho da barra.
                    let (rw, rh, ox, oy, iw, ih) = match self.slot.lock() {
                        Ok(s) if s.rw > 0 && s.w > 0 => {
                            (s.rw as f64, s.rh as f64, s.ox as f64, s.oy as f64,
                             s.w as f64, s.h as f64)
                        }
                        _ => return,
                    };
                    let Some((x, y)) =
                        ponto_remoto(position.x, position.y, (ox, oy, iw, ih, rw, rh))
                    else {
                        return;
                    };
                    self.manda(Evento::Mouse { x, y });
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let codigo = match button {
                        MouseButton::Left => BTN_LEFT,
                        MouseButton::Right => BTN_RIGHT,
                        MouseButton::Middle => BTN_MIDDLE,
                        _ => return,
                    };
                    self.manda(Evento::Botao {
                        codigo,
                        apertado: state == ElementState::Pressed,
                    });
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    // O trackpad do Mac manda pixels; a roda manda
                    // linhas. O Mutter quer um deslocamento continuo,
                    // entao a linha vira um passo de ~15 px.
                    let (dx, dy) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (-x as f64 * 15.0, -y as f64 * 15.0),
                        MouseScrollDelta::PixelDelta(p) => (-p.x, -p.y),
                    };
                    self.manda(Evento::Rolagem { dx, dy });
                }
                WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                    // Sintetico e o que o sistema inventa ao focar a
                    // janela; repassar isso apertaria tecla que ninguem
                    // apertou.
                    if is_synthetic {
                        return;
                    }
                    let PhysicalKey::Code(code) = event.physical_key else { return };
                    let Some(keycode) = evdev_de_winit(code) else { return };
                    self.manda(Evento::Tecla {
                        keycode,
                        apertado: event.state == ElementState::Pressed,
                    });
                }
                _ => {}
            }
        }

        /// A rede avisou que chegou quadro.
        ///
        /// Sem isto a imagem CONGELAVA. O laco usa `ControlFlow::Wait`,
        /// que bloqueia esperando evento do sistema; a thread de rede
        /// enchia o slot e nao acordava ninguem. O `about_to_wait` roda
        /// ANTES de o laco dormir, entao o quadro que chegasse depois
        /// esperava o proximo evento do sistema -- um movimento de mouse,
        /// uma tecla. O usuario descreveu exatamente isso: "congela numa
        /// mensagem do chat ate eu mexer o mouse".
        ///
        /// Pedir redesenho aqui e nao desenhar direto: desenhar fora do
        /// `RedrawRequested` briga com o compositor em algumas
        /// plataformas, e o caminho de pintura ja existe num lugar so.
        fn user_event(&mut self, _: &ActiveEventLoop, _: ()) {
            if let Some(j) = &self.janela {
                j.request_redraw();
            }
        }

        fn about_to_wait(&mut self, _: &ActiveEventLoop) {
            // Rede de seguranca para o quadro que chegou entre o ultimo
            // desenho e o laco dormir. O aviso do `user_event` e o
            // caminho normal; esta comparacao cobre a corrida.
            let g = self.slot.lock().map(|s| s.geracao).unwrap_or(0);
            if g != self.desenhada {
                if let Some(j) = &self.janela {
                    j.request_redraw();
                }
            }
        }
    }

    impl App {
        /// `try_send` e nao `send`: o laco de janela nao pode bloquear
        /// esperando a rede -- travaria a interface inteira.
        fn manda(&self, e: Evento) {
            let _ = self.tx_in.try_send(e);
        }

        fn desenhar(&mut self) {
            let (Some(janela), Some(surface)) = (self.janela.as_ref(), self.surface.as_mut())
            else {
                return;
            };
            // Primeiro garante que a thread de escala conhece o tamanho
            // atual: na primeira pintura ainda nao houve `Resized`.
            let fisico = janela.inner_size();
            if let Ok(mut g) = self.tam.lock() {
                *g = (fisico.width.max(1), fisico.height.max(1));
            }
            let s = match self.slot.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.w == 0 || s.h == 0 {
                return;
            }
            // A superficie e do tamanho da JANELA, nao do quadro.
            //
            // Antes eu dimensionava pelo quadro e ABANDONAVA a pintura
            // quando os dois nao batiam. Ao entrar em tela cheia a janela
            // crescia, todo quadro em voo era descartado, e o que ficava
            // na tela era o buffer antigo -- pequeno, no canto superior
            // esquerdo, sem esticar. Era o que o usuario via.
            //
            // Agora pinta SEMPRE: a janela recebe fundo, a imagem vai no
            // lugar dela, e um quadro de tamanho velho aparece
            // ligeiramente fora de escala por um quadro em vez de travar
            // a tela. Nao existe mais estado preso.
            let (Some(sw), Some(sh)) =
                (NonZeroU32::new(fisico.width.max(1)), NonZeroU32::new(fisico.height.max(1)))
            else {
                return;
            };
            if surface.resize(sw, sh).is_err() {
                return;
            }
            let Ok(mut buf) = surface.buffer_mut() else { return };
            let (jw, jh) = (fisico.width as usize, fisico.height as usize);
            buf.fill(FUNDO);
            let (ox, oy) = (s.ox as usize, s.oy as usize);
            let (iw, ih) = (s.w as usize, s.h as usize);
            for y in 0..ih {
                let dy = oy + y;
                if dy >= jh {
                    break;
                }
                let largura = iw.min(jw.saturating_sub(ox));
                let origem = y * iw;
                let destino = dy * jw + ox;
                if origem + largura > s.rgb.len() || destino + largura > buf.len() {
                    break;
                }
                buf[destino..destino + largura]
                    .copy_from_slice(&s.rgb[origem..origem + largura]);
            }
            self.desenhada = s.geracao;
            let capturado = s.capturado_ns;
            drop(s);
            let _ = buf.present();
            if self.medir {
                self.atrasos.push(
                    logistream::serve::agora_ns().saturating_sub(capturado) as f64 / 1e6,
                );
                if self.atrasos.len() == 120 {
                    self.atrasos.sort_by(f64::total_cmp);
                    eprintln!(
                        "captura ate a tela: p50 {:.1} ms, p95 {:.1} ms",
                        self.atrasos[60], self.atrasos[114]
                    );
                    self.atrasos.clear();
                }
            }

            self.n += 1;
            if self.n % 120 == 0 {
                let seg = self.t0.elapsed().as_secs_f64();
                // No log tambem, nao so no titulo: foi justamente esta
                // taxa que ficou em zero enquanto a de decodificacao
                // subia, e um numero que so existe no titulo da janela
                // nao serve para medir nada remotamente.
                println!("  {} quadros desenhados, {:.0} fps", self.n, self.n as f64 / seg);
                janela.set_title(&format!(
                    "logistream — {:.0} fps desenhados",
                    self.n as f64 / seg
                ));
            }
        }
    }

    /// Recebe, remonta, decodifica e deixa no slot.
    #[tokio::main(flavor = "current_thread")]
    async fn receber(
        alvo: &str,
        slot: Arc<Mutex<Slot>>,
        mut rx_in: tokio::sync::mpsc::Receiver<Evento>,
        tam: Arc<Mutex<(u32, u32)>>,
        proxy: winit::event_loop::EventLoopProxy<()>,
    ) -> anyhow::Result<()> {
        // Decodificador montado ANTES de conectar.
        //
        // Ele ficava depois da conexao, e isso perdia o primeiro
        // quadro-chave: o host comeca a despejar o IDR no instante do
        // handshake, e enquanto montavamos o decodificador os datagramas
        // iam chegando. O que nao cabia na fila era descartado, o IDR
        // chegava incompleto, e o log enchia de `non-existing PPS 0
        // referenced`.
        let mut dec = logistream::decode::Decodificador::novo()
            .map_err(|e| anyhow::anyhow!("sem decodificador: {e}"))?;
        println!("decodificando por {}", dec.nome());

        let mut enderecos: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host(alvo).await?.collect();
        // IPv4 na frente: nome `.local` resolve nas duas familias e o
        // socket local tem de nascer da familia do destino.
        enderecos.sort_by_key(|a| !a.is_ipv4());
        let destino = *enderecos.first().ok_or_else(|| anyhow::anyhow!("nao resolvi {alvo}"))?;
        let bind: std::net::SocketAddr =
            if destino.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
        let ep = net::cliente(bind)?;
        let conexao = ep.connect(destino, "logistream")?.await?;
        println!("conectado em {destino}");

        // Input por stream unidirecional: confiavel e ordenado. Perder
        // um "tecla solta" deixaria a tecla presa no host.
        //
        // O `open_uni` entra na tarefa, e nao aqui: ele custa uma ida e
        // volta, e pagar isso antes do laco de datagramas era perder o
        // comeco do video. Quem digita no primeiro milissegundo nao
        // existe; quem perde o quadro-chave ve tela preta.
        let conexao_in = conexao.clone();
        tokio::spawn(async move {
            let Ok(mut envio) = conexao_in.open_uni().await else { return };
            let mut buf = bytes::BytesMut::new();
            while let Some(e) = rx_in.recv().await {
                buf.clear();
                e.escrever(&mut buf);
                if envio.write_all(&buf).await.is_err() {
                    break;
                }
            }
        });

        let mut r = Reassembler::default();
        let mut quadros = 0u64;

        loop {
            let d = match conexao.read_datagram().await {
                Ok(d) => d,
                Err(e) => {
                    println!("conexao encerrada: {e}");
                    return Ok(());
                }
            };
            let Some((cab, unidade)) = r.push(&d) else { continue };
            // Os primeiros quadros, um a um.
            //
            // "A imagem nao aparece" tem causas diferentes com sintomas
            // iguais, e so estes tres numeros as separam: se o primeiro
            // quadro inteiro NAO e chave, o host mandou P para um cliente
            // sem contexto; se e chave mas `perdidos` ja subiu, ele veio
            // picado; se esta tudo certo aqui, o problema e do
            // decodificador.
            if quadros < 3 {
                println!(
                    "  recebi quadro {} {} {} bytes, {} perdido(s) ate aqui",
                    cab.frame,
                    if cab.keyframe { "CHAVE" } else { "P" },
                    unidade.len(),
                    r.perdidos
                );
            }

            // O encaixe depende do tamanho da ORIGEM, que so o
            // decodificador conhece, e do da JANELA, que so a interface
            // conhece. Dai passar a conta como funcao em vez de um
            // numero: cada lado contribui com o que sabe.
            let tam_agora = Arc::clone(&tam);
            let mut encaixe = move |rw: u32, rh: u32| {
                let (jw, jh) = tam_agora.lock().map(|g| *g).unwrap_or((rw, rh));
                encaixa(rw, rh, jw.max(1), jh.max(1))
            };
            let prontos = match dec.alimentar(&unidade, &mut encaixe) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("decodificar: {e}");
                    continue;
                }
            };
            for pr in prontos {
                if let Ok(mut s) = slot.lock() {
                    let (jw, jh) = tam.lock().map(|g| *g).unwrap_or((pr.w, pr.h));
                    s.rgb = pr.rgb;
                    s.w = pr.w;
                    s.h = pr.h;
                    s.ox = pr.ox;
                    s.oy = pr.oy;
                    s.rw = pr.rw;
                    s.rh = pr.rh;
                    s.jw = jw;
                    s.jh = jh;
                    s.geracao += 1;
                    s.capturado_ns = cab.captured_at_ns;
                }
                // Acorda a interface. `Err` aqui significa que o laco da
                // janela acabou -- a pessoa fechou -- e ai nao ha mais
                // para quem desenhar.
                if proxy.send_event(()).is_err() {
                    return Ok(());
                }
                quadros += 1;
                if quadros % 120 == 0 {
                    println!("  {quadros} quadros decodificados");
                }
            }
        }
    }
}
