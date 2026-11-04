//! Eventos de entrada que o cliente manda de volta ao host.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Por que stream e nao datagrama
//!
//! Video tolera perda; input nao. Soltar um "tecla solta" deixa a tecla
//! presa no host, e soltar um "botao solto" deixa o mouse arrastando
//! sozinho. Por isso o input vai por **stream** QUIC -- confiavel e
//! ordenado -- enquanto o video vai por datagrama.
//!
//! ## Formato fixo por tipo
//!
//! Um byte de tipo e um tamanho conhecido por tipo. Sem campo de
//! comprimento: o leitor sabe quantos bytes pedir assim que le o tipo, e
//! nao ha como um numero errado fazer ele ler o evento seguinte no meio.

use bytes::{Buf, BufMut, BytesMut};

/// Codigos de botao do evdev, que e o que o Mutter espera.
pub const BTN_LEFT: i32 = 0x110;
pub const BTN_RIGHT: i32 = 0x111;
pub const BTN_MIDDLE: i32 = 0x112;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Evento {
    /// Posicao absoluta no espaco do stream capturado (pixels).
    ///
    /// Absoluto e nao relativo porque o cliente sabe exatamente onde o
    /// cursor dele esta dentro da janela, e converter para relativo
    /// acumularia erro a cada quadro.
    Mouse { x: f64, y: f64 },
    Botao { codigo: i32, apertado: bool },
    Rolagem { dx: f64, dy: f64 },
    /// Keycode do evdev (o que o `NotifyKeyboardKeycode` recebe).
    Tecla { keycode: u32, apertado: bool },
}

impl Evento {
    const T_MOUSE: u8 = 1;
    const T_BOTAO: u8 = 2;
    const T_ROLAGEM: u8 = 3;
    const T_TECLA: u8 = 4;

    /// Bytes de payload de cada tipo, sem contar o byte do tipo.
    fn tamanho(tag: u8) -> Option<usize> {
        Some(match tag {
            Self::T_MOUSE => 16,
            Self::T_BOTAO => 5,
            Self::T_ROLAGEM => 16,
            Self::T_TECLA => 5,
            _ => return None,
        })
    }

    pub fn escrever(&self, b: &mut BytesMut) {
        match *self {
            Evento::Mouse { x, y } => {
                b.put_u8(Self::T_MOUSE);
                b.put_f64(x);
                b.put_f64(y);
            }
            Evento::Botao { codigo, apertado } => {
                b.put_u8(Self::T_BOTAO);
                b.put_i32(codigo);
                b.put_u8(u8::from(apertado));
            }
            Evento::Rolagem { dx, dy } => {
                b.put_u8(Self::T_ROLAGEM);
                b.put_f64(dx);
                b.put_f64(dy);
            }
            Evento::Tecla { keycode, apertado } => {
                b.put_u8(Self::T_TECLA);
                b.put_u32(keycode);
                b.put_u8(u8::from(apertado));
            }
        }
    }
}

/// Junta bytes do stream e entrega eventos inteiros.
///
/// Stream nao respeita fronteira de mensagem: um `read` pode trazer meio
/// evento ou tres e meio. Sem este acumulador, meio evento viraria
/// comando errado -- e comando errado aqui mexe no computador de alguem.
#[derive(Default)]
pub struct Leitor {
    buf: BytesMut,
    /// Bytes descartados por tipo desconhecido. Se isto cresce, as duas
    /// pontas estao em versoes diferentes do protocolo.
    pub desconhecidos: u64,
}

impl Leitor {
    pub fn alimentar(&mut self, dados: &[u8]) {
        self.buf.extend_from_slice(dados);
    }

    pub fn proximo(&mut self) -> Option<Evento> {
        loop {
            if self.buf.is_empty() {
                return None;
            }
            let tag = self.buf[0];
            let Some(n) = Evento::tamanho(tag) else {
                // Tipo que nao conhecemos: pular 1 byte e tentar de novo
                // e melhor que travar o stream para sempre.
                self.buf.advance(1);
                self.desconhecidos += 1;
                continue;
            };
            if self.buf.len() < 1 + n {
                return None; // evento incompleto: espera mais bytes
            }
            self.buf.advance(1);
            let e = match tag {
                Evento::T_MOUSE => {
                    Evento::Mouse { x: self.buf.get_f64(), y: self.buf.get_f64() }
                }
                Evento::T_BOTAO => Evento::Botao {
                    codigo: self.buf.get_i32(),
                    apertado: self.buf.get_u8() != 0,
                },
                Evento::T_ROLAGEM => {
                    Evento::Rolagem { dx: self.buf.get_f64(), dy: self.buf.get_f64() }
                }
                _ => Evento::Tecla {
                    keycode: self.buf.get_u32(),
                    apertado: self.buf.get_u8() != 0,
                },
            };
            return Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todos() -> Vec<Evento> {
        vec![
            Evento::Mouse { x: 1280.5, y: 720.25 },
            Evento::Botao { codigo: BTN_LEFT, apertado: true },
            Evento::Botao { codigo: BTN_LEFT, apertado: false },
            Evento::Rolagem { dx: 0.0, dy: -3.5 },
            Evento::Tecla { keycode: 30, apertado: true },
        ]
    }

    #[test]
    fn vai_e_volta() {
        let mut b = BytesMut::new();
        for e in todos() {
            e.escrever(&mut b);
        }
        let mut l = Leitor::default();
        l.alimentar(&b);
        let lidos: Vec<_> = std::iter::from_fn(|| l.proximo()).collect();
        assert_eq!(lidos, todos());
        assert_eq!(l.desconhecidos, 0);
    }

    #[test]
    fn stream_partido_no_meio_do_evento() {
        // O caso que importa: TCP/QUIC stream nao respeita fronteira de
        // mensagem. Meio evento interpretado seria comando errado na
        // maquina de alguem.
        let mut b = BytesMut::new();
        for e in todos() {
            e.escrever(&mut b);
        }
        let bytes = b.to_vec();
        let mut l = Leitor::default();
        let mut lidos = Vec::new();
        // Um byte por vez: o pior fatiamento possivel.
        for byte in &bytes {
            l.alimentar(&[*byte]);
            while let Some(e) = l.proximo() {
                lidos.push(e);
            }
        }
        assert_eq!(lidos, todos());
    }

    #[test]
    fn tipo_desconhecido_nao_trava_o_stream() {
        // Versoes diferentes nas duas pontas: o que nao se entende e
        // pulado, e o que vem depois continua valendo.
        let mut b = BytesMut::new();
        b.put_u8(99); // tipo do futuro
        Evento::Tecla { keycode: 57, apertado: true }.escrever(&mut b);
        let mut l = Leitor::default();
        l.alimentar(&b);
        assert_eq!(l.proximo(), Some(Evento::Tecla { keycode: 57, apertado: true }));
        assert_eq!(l.desconhecidos, 1);
    }
}

/// Tradutor de tecla fisica para keycode do evdev.
///
/// O `winit` entrega `KeyCode`, que segue os nomes do W3C (`KeyA`,
/// `Digit1`); o Mutter quer o numero do evdev. Nao ha formula: e tabela.
///
/// **Fisica e nao logica** de proposito. Mandar o caractere faria o
/// layout do cliente mandar no host -- apertar `A` num teclado ABNT2
/// viraria outra coisa numa maquina com layout americano. Mandando a
/// POSICAO da tecla, cada maquina aplica o proprio layout, que e como um
/// teclado de verdade se comporta.
#[cfg(feature = "winit-keys")]
pub fn evdev_de_winit(code: winit::keyboard::KeyCode) -> Option<u32> {
    use winit::keyboard::KeyCode as K;
    Some(match code {
        K::Escape => 1,
        K::Digit1 => 2, K::Digit2 => 3, K::Digit3 => 4, K::Digit4 => 5, K::Digit5 => 6,
        K::Digit6 => 7, K::Digit7 => 8, K::Digit8 => 9, K::Digit9 => 10, K::Digit0 => 11,
        K::Minus => 12, K::Equal => 13, K::Backspace => 14, K::Tab => 15,
        K::KeyQ => 16, K::KeyW => 17, K::KeyE => 18, K::KeyR => 19, K::KeyT => 20,
        K::KeyY => 21, K::KeyU => 22, K::KeyI => 23, K::KeyO => 24, K::KeyP => 25,
        K::BracketLeft => 26, K::BracketRight => 27, K::Enter => 28, K::ControlLeft => 29,
        K::KeyA => 30, K::KeyS => 31, K::KeyD => 32, K::KeyF => 33, K::KeyG => 34,
        K::KeyH => 35, K::KeyJ => 36, K::KeyK => 37, K::KeyL => 38,
        K::Semicolon => 39, K::Quote => 40, K::Backquote => 41, K::ShiftLeft => 42,
        K::Backslash => 43,
        K::KeyZ => 44, K::KeyX => 45, K::KeyC => 46, K::KeyV => 47, K::KeyB => 48,
        K::KeyN => 49, K::KeyM => 50,
        K::Comma => 51, K::Period => 52, K::Slash => 53, K::ShiftRight => 54,
        K::NumpadMultiply => 55, K::AltLeft => 56, K::Space => 57, K::CapsLock => 58,
        K::F1 => 59, K::F2 => 60, K::F3 => 61, K::F4 => 62, K::F5 => 63,
        K::F6 => 64, K::F7 => 65, K::F8 => 66, K::F9 => 67, K::F10 => 68,
        K::NumLock => 69, K::ScrollLock => 70,
        K::Numpad7 => 71, K::Numpad8 => 72, K::Numpad9 => 73, K::NumpadSubtract => 74,
        K::Numpad4 => 75, K::Numpad5 => 76, K::Numpad6 => 77, K::NumpadAdd => 78,
        K::Numpad1 => 79, K::Numpad2 => 80, K::Numpad3 => 81, K::Numpad0 => 82,
        K::NumpadDecimal => 83,
        K::IntlBackslash => 86, K::F11 => 87, K::F12 => 88,
        // A tecla a direita do `M` em teclado ABNT2.
        K::IntlRo => 89,
        K::NumpadEnter => 96, K::ControlRight => 97, K::NumpadDivide => 98,
        K::PrintScreen => 99, K::AltRight => 100,
        K::Home => 102, K::ArrowUp => 103, K::PageUp => 104, K::ArrowLeft => 105,
        K::ArrowRight => 106, K::End => 107, K::ArrowDown => 108, K::PageDown => 109,
        K::Insert => 110, K::Delete => 111,
        K::AudioVolumeMute => 113, K::AudioVolumeDown => 114, K::AudioVolumeUp => 115,
        K::Pause => 119,
        K::SuperLeft => 125, K::SuperRight => 126, K::ContextMenu => 127,
        _ => return None,
    })
}

/// Traduz keycode do evdev para scancode do Set 1 do AT.
///
/// Devolve `(scancode, precisa do prefixo E0)`. Quem usa hoje e o host
/// do Windows, que injeta por `KEYEVENTF_SCANCODE` -- mas a tabela mora
/// AQUI, junto do seu inverso (`evdev_de_winit`), e nao no modulo de
/// plataforma: ela e tabela pura, e no modulo `cfg(windows)` os testes
/// dela nunca rodariam na maquina onde ela e escrita. Tabela sem teste
/// que roda e tabela que erra uma tecla em silencio.
///
/// Quase identidade por um acidente feliz: os 83 primeiros codigos do
/// evdev foram COPIADOS do Set 1 (evdev 30 = `A` = 0x1E = 30). O que a
/// tabela trata e o que fugiu disso -- as teclas de prefixo `E0` (setas,
/// Home, Ctrl direito, as duas de Windows) e as de layout ABNT2.
pub fn scancode_de_evdev(keycode: u32) -> Option<(u16, bool)> {
    Some(match keycode {
        // Identidade: os codigos do evdev ate 88 sao os do Set 1.
        // 84 e 85 nao existem em teclado comum e o cliente nao os manda.
        1..=83 | 86..=88 => (keycode as u16, false),
        // KEY_RO: a tecla a direita do `M` no ABNT2 e no japones.
        89 => (0x73, false),
        96 => (0x1C, true),  // Enter do teclado numerico
        97 => (0x1D, true),  // Ctrl direito
        98 => (0x35, true),  // / do numerico
        99 => (0x37, true),  // PrintScreen
        100 => (0x38, true), // Alt direito (AltGr)
        102 => (0x47, true), // Home
        103 => (0x48, true), // seta cima
        104 => (0x49, true), // PageUp
        105 => (0x4B, true), // seta esquerda
        106 => (0x4D, true), // seta direita
        107 => (0x4F, true), // End
        108 => (0x50, true), // seta baixo
        109 => (0x51, true), // PageDown
        110 => (0x52, true), // Insert
        111 => (0x53, true), // Delete
        113 => (0x20, true), // mudo
        114 => (0x2E, true), // volume -
        115 => (0x30, true), // volume +
        125 => (0x5B, true), // Windows esquerdo
        126 => (0x5C, true), // Windows direito
        127 => (0x5D, true), // menu de contexto
        // 119 (Pause) sai por tecla virtual: o scancode dele e a
        // sequencia de prefixo `E1 1D 45`, que o SendInput nao expressa.
        _ => return None,
    })
}

#[cfg(all(test, feature = "winit-keys"))]
mod testes_teclas {
    use super::*;
    use winit::keyboard::KeyCode as K;

    #[test]
    fn teclas_comuns_tem_codigo() {
        // Uma amostra que cobre letra, digito, modificador, seta e funcao.
        for (k, esperado) in [
            (K::KeyA, 30),
            (K::Digit1, 2),
            (K::Space, 57),
            (K::Enter, 28),
            (K::ShiftLeft, 42),
            (K::ControlLeft, 29),
            (K::SuperLeft, 125),
            (K::ArrowUp, 103),
            (K::F5, 63),
            (K::Escape, 1),
        ] {
            assert_eq!(evdev_de_winit(k), Some(esperado), "{k:?}");
        }
    }

    #[test]
    fn tecla_sem_mapeamento_devolve_nada() {
        // Melhor ignorar que mandar codigo errado: codigo errado aperta
        // OUTRA tecla na maquina de alguem.
        assert_eq!(evdev_de_winit(K::Fn), None);
    }
}

#[cfg(test)]
mod testes_scancode {
    use super::*;

    #[test]
    fn letras_e_digitos_sao_identidade() {
        // O acidente historico que a tabela depende: evdev 1..83 == Set 1.
        assert_eq!(scancode_de_evdev(30), Some((0x1E, false))); // A
        assert_eq!(scancode_de_evdev(2), Some((0x02, false))); // 1
        assert_eq!(scancode_de_evdev(57), Some((0x39, false))); // espaco
        assert_eq!(scancode_de_evdev(28), Some((0x1C, false))); // Enter
    }

    #[test]
    fn teclas_estendidas_pedem_prefixo() {
        // Sem o E0, `seta cima` viraria `8` do numerico -- mesmo
        // scancode, significado diferente.
        assert_eq!(scancode_de_evdev(103), Some((0x48, true)));
        assert_eq!(scancode_de_evdev(72), Some((0x48, false)));
        assert_eq!(scancode_de_evdev(97), Some((0x1D, true))); // Ctrl dir
        assert_eq!(scancode_de_evdev(29), Some((0x1D, false))); // Ctrl esq
    }


    /// Codigos do evdev que a tabela NAO traduz, e por que.
    ///
    /// Lista explicita de proposito: e ela que faz o teste de cobertura
    /// falhar quando alguem ensinar o cliente a mandar uma tecla nova e
    /// esquecer o lado do host. Sem a lista, o buraco seria "normal".
    const SEM_TRADUCAO: &[(u32, &str)] = &[
        (84, "nao existe em teclado comum"),
        (85, "zenkaku/hankaku, teclado japones"),
        (90, "katakana"),
        (91, "hiragana"),
        (92, "henkan"),
        (93, "katakana/hiragana"),
        (94, "muhenkan"),
        (95, "ponto do numerico japones"),
        (101, "linefeed, nao existe em teclado fisico atual"),
        (112, "macro"),
        (116, "power"),
        (117, "= do numerico"),
        (118, "+/- do numerico"),
        (119, "Pause: o scancode dele e a sequencia E1 1D 45; vai por tecla virtual"),
        (120, "scale"),
        (121, "virgula do numerico"),
        (122, "hangeul"),
        (123, "hanja"),
        (124, "yen"),
    ];

    #[test]
    fn toda_tecla_que_o_cliente_manda_tem_traducao() {
        // A invariante que importa: se o visualizador consegue mandar um
        // codigo, o host do Windows tem de saber o que fazer com ele.
        // Um buraco aqui nao da erro -- a tecla simplesmente nao faz
        // nada, e descobrir isso e no teclado de alguem.
        for c in 1u32..=127 {
            let excecao = SEM_TRADUCAO.iter().find(|(k, _)| *k == c);
            match (scancode_de_evdev(c), excecao) {
                (Some(_), None) => {}
                (None, Some(_)) => {}
                (Some(_), Some((_, motivo))) => {
                    panic!("evdev {c} esta traduzido mas listado como sem traducao ({motivo})")
                }
                (None, None) => panic!(
                    "evdev {c} nao tem scancode e nao esta na lista de excecoes -- \
                     se o cliente mandar essa tecla, ela nao faz nada no Windows"
                ),
            }
        }
    }

    #[test]
    fn duas_teclas_nao_viram_a_mesma() {
        // Colisao seria pior que buraco: a tecla errada aperta OUTRA
        // tecla na maquina de alguem, e o sintoma nao aponta para aqui.
        let mut visto: std::collections::BTreeMap<(u16, bool), u32> = Default::default();
        for c in 1u32..=127 {
            if let Some(par) = scancode_de_evdev(c) {
                if let Some(antes) = visto.insert(par, c) {
                    panic!("evdev {antes} e {c} geram o mesmo scancode {par:?}");
                }
            }
        }
    }

    #[test]
    fn pause_nao_tem_scancode() {
        // Tratada por tecla virtual; devolver Some aqui injetaria outra.
        assert_eq!(scancode_de_evdev(119), None);
    }
}
