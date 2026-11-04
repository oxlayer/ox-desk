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
//! Onde ha `libavcodec`: macOS (brew) e Linux (apt). Fora disso o `main`
//! e um recado em vez de um erro de link, porque o binario e construido
//! junto dos outros e nao vale quebrar o build todo por ele. O Windows
//! ainda cai aqui -- ele ja SERVE a propria tela, mas nao assiste.
//!
//! Entre as duas ha **um slot**, nao uma fila: quadro novo sobrescreve o
//! que ainda nao foi desenhado. Fila so acumularia atraso, e numa tela
//! remota quadro velho nao tem valor -- ja passou.
//!
//!   cargo run --release --bin view -- <host:porta>

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!(
        "o visualizador precisa de libavcodec, e aqui nao ha: ele compila \
         no macOS (brew) e no Linux (apt). Ver specs/010."
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    visual::run()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

    /// O quadro pronto para desenhar, e o tamanho dele.
    #[derive(Default)]
    pub struct Slot {
        pub rgb: Vec<u32>,
        pub w: u32,
        pub h: u32,
        /// Sobe a cada quadro novo; a janela so redesenha quando muda.
        pub geracao: u64,
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

        std::thread::spawn(move || {
            if let Err(e) = receber(&alvo, slot_rede, rx_in, tam_rede) {
                eprintln!("rede: {e}");
                std::process::exit(1);
            }
        });

        let ev = EventLoop::new()?;
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
                    // A janela pode estar menor que a tela remota, entao
                    // a posicao vai em coordenadas DO STREAM -- e o host
                    // que tem o espaco de coordenadas, nao nos.
                    let (sw, sh) = match self.slot.lock() {
                        Ok(s) if s.w > 0 => (s.w as f64, s.h as f64),
                        _ => return,
                    };
                    let (jw, jh) = self.tam_janela;
                    if jw <= 0.0 || jh <= 0.0 {
                        return;
                    }
                    self.manda(Evento::Mouse {
                        x: position.x / jw * sw,
                        y: position.y / jh * sh,
                    });
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

        fn about_to_wait(&mut self, _: &ActiveEventLoop) {
            // Acorda a janela so quando ha quadro novo. Comparar geracao
            // evita redesenhar a mesma imagem e queimar GPU a toa.
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
            // O quadro so serve se tiver o tamanho da janela AGORA; se a
            // janela mudou de tamanho, o proximo quadro ja vem certo.
            if s.w == 0 || s.h == 0 || s.w != fisico.width || s.h != fisico.height {
                return;
            }
            let (Some(w), Some(h)) = (NonZeroU32::new(s.w), NonZeroU32::new(s.h)) else {
                return;
            };
            if surface.resize(w, h).is_err() {
                return;
            }
            let Ok(mut buf) = surface.buffer_mut() else { return };
            let n = buf.len().min(s.rgb.len());
            buf[..n].copy_from_slice(&s.rgb[..n]);
            self.desenhada = s.geracao;
            drop(s);
            let _ = buf.present();

            self.n += 1;
            if self.n % 120 == 0 {
                let seg = self.t0.elapsed().as_secs_f64();
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
    ) -> anyhow::Result<()> {
        // Decodificador montado ANTES de conectar.
        //
        // Ele ficava depois da conexao, e isso perdia o primeiro
        // quadro-chave: o host comeca a despejar o IDR no instante do
        // handshake, e enquanto montavamos o decodificador (e esperavamos
        // uma ida e volta para abrir o stream de input) os datagramas
        // iam chegando. O que nao cabia na fila era descartado, o IDR
        // chegava incompleto, e o log enchia de `non-existing PPS 0
        // referenced`. Reproduzia ao reconectar, porque ai o host ja
        // esta quente e dispara na hora.
        ffmpeg_next::init()?;
        let codec = ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::H264)
            .ok_or_else(|| anyhow::anyhow!("sem decoder H.264"))?;
        let ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec);
        let mut dec = ctx.decoder().video()?;

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
        let mut escalador: Option<ffmpeg_next::software::scaling::Context> = None;
        let mut escala_de_para = (0u32, 0u32, 0u32, 0u32);
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
            // decodificador. Sem isto eu passei a gastar palpites.
            if quadros < 3 {
                println!(
                    "  recebi quadro {} {} {} bytes, {} perdido(s) ate aqui",
                    cab.frame,
                    if cab.keyframe { "CHAVE" } else { "P" },
                    unidade.len(),
                    r.perdidos
                );
            }

            let pacote = ffmpeg_next::Packet::copy(&unidade);
            if dec.send_packet(&pacote).is_err() {
                continue;
            }
            let mut quadro = ffmpeg_next::frame::Video::empty();
            while dec.receive_frame(&mut quadro).is_ok() {
                let (w, h) = (quadro.width(), quadro.height());
                // Destino = tamanho da JANELA. O `swscale` ja estava no
                // caminho fazendo conversao de cor; redimensionar junto
                // nao custa outra passada sobre os pixels.
                let (jw, jh) = tam.lock().map(|g| *g).unwrap_or((w, h));
                let (jw, jh) = (jw.max(1), jh.max(1));
                // Recriar o escalador e caro; so quando de fato mudou.
                let agora = (w, h, jw, jh);
                if escalador.is_none() || escala_de_para != agora {
                    escalador = ffmpeg_next::software::scaling::Context::get(
                        quadro.format(),
                        w,
                        h,
                        ffmpeg_next::format::Pixel::BGRA,
                        jw,
                        jh,
                        ffmpeg_next::software::scaling::Flags::BILINEAR,
                    )
                    .ok();
                    escala_de_para = agora;
                }
                let Some(sc) = escalador.as_mut() else { continue };
                let mut rgba = ffmpeg_next::frame::Video::empty();
                if sc.run(&quadro, &mut rgba).is_err() {
                    continue;
                }

                // BGRA em bytes vira 0xAARRGGBB em little-endian, que e o
                // que o softbuffer espera (o byte alto e ignorado).
                let linha = rgba.stride(0);
                let dados = rgba.data(0);
                let (w, h) = (jw, jh);
                let mut saida = vec![0u32; (w * h) as usize];
                for y in 0..h as usize {
                    let ini = y * linha;
                    for x in 0..w as usize {
                        let p = ini + x * 4;
                        if p + 3 < dados.len() {
                            saida[y * w as usize + x] = u32::from_le_bytes([
                                dados[p],
                                dados[p + 1],
                                dados[p + 2],
                                dados[p + 3],
                            ]);
                        }
                    }
                }

                if let Ok(mut s) = slot.lock() {
                    s.rgb = saida;
                    s.w = w;
                    s.h = h;
                    s.geracao += 1;
                }
                quadros += 1;
                if quadros % 120 == 0 {
                    println!("  {quadros} quadros decodificados");
                }
            }
        }
    }
}
