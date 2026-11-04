//! Visualizador: QUIC -> H.264 -> janela. Fecha a v2.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Onde cada coisa roda
//!
//! Rede e decodificacao numa thread, desenho na thread da interface. O
//! laco de janela do macOS e dono da thread principal e nao pode
//! bloquear -- se decodificasse ali, cada quadro travaria a interface.
//!
//! Entre as duas ha **um slot**, nao uma fila: quadro novo sobrescreve o
//! que ainda nao foi desenhado. Fila so acumularia atraso, e numa tela
//! remota quadro velho nao tem valor -- ja passou.
//!
//!   cargo run --release --bin view -- <host:porta>

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "o visualizador so compila no macOS por enquanto: ele usa o \
         libavcodec do brew e a janela do AppKit. Ver specs/010."
    );
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    mac::run()
}

#[cfg(target_os = "macos")]
mod mac {
    use std::num::NonZeroU32;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use logistream::net::{self, Reassembler};
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

        std::thread::spawn(move || {
            if let Err(e) = receber(&alvo, slot_rede) {
                eprintln!("rede: {e}");
                std::process::exit(1);
            }
        });

        let ev = EventLoop::new()?;
        // `Wait` e nao `Poll`: acordamos quando chega quadro, nao a
        // 100% de CPU perguntando se chegou.
        ev.set_control_flow(ControlFlow::Wait);
        let mut app = App { janela: None, surface: None, slot, desenhada: 0, n: 0, t0: Instant::now() };
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
        fn desenhar(&mut self) {
            let (Some(janela), Some(surface)) = (self.janela.as_ref(), self.surface.as_mut())
            else {
                return;
            };
            let s = match self.slot.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.w == 0 || s.h == 0 {
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
    async fn receber(alvo: &str, slot: Arc<Mutex<Slot>>) -> anyhow::Result<()> {
        ffmpeg_next::init()?;

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

        let codec = ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::H264)
            .ok_or_else(|| anyhow::anyhow!("sem decoder H.264"))?;
        let ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec);
        let mut dec = ctx.decoder().video()?;

        let mut r = Reassembler::default();
        let mut escalador: Option<ffmpeg_next::software::scaling::Context> = None;
        let mut quadros = 0u64;

        loop {
            let d = match conexao.read_datagram().await {
                Ok(d) => d,
                Err(e) => {
                    println!("conexao encerrada: {e}");
                    return Ok(());
                }
            };
            let Some((_cab, unidade)) = r.push(&d) else { continue };

            let pacote = ffmpeg_next::Packet::copy(&unidade);
            if dec.send_packet(&pacote).is_err() {
                continue;
            }
            let mut quadro = ffmpeg_next::frame::Video::empty();
            while dec.receive_frame(&mut quadro).is_ok() {
                let (w, h) = (quadro.width(), quadro.height());
                // O escalador e caro de criar; so refaz se o formato
                // mudou. Ele tambem faz a conversao de cor, que e o
                // trabalho de CPU que o VideoToolbox vai eliminar depois.
                let precisa = escalador
                    .as_ref()
                    .map(|s| s.input().width != w || s.input().height != h)
                    .unwrap_or(true);
                if precisa {
                    escalador = ffmpeg_next::software::scaling::Context::get(
                        quadro.format(),
                        w,
                        h,
                        ffmpeg_next::format::Pixel::BGRA,
                        w,
                        h,
                        ffmpeg_next::software::scaling::Flags::BILINEAR,
                    )
                    .ok();
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
