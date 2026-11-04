//! Aplicar input no Windows: `SendInput`.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Tecla vai por SCANCODE, nao por tecla virtual
//!
//! O protocolo carrega keycode do evdev, que e a POSICAO fisica da tecla
//! (ver `input::evdev_de_winit`). Aqui ela e traduzida para scancode do
//! Set 1 e injetada com `KEYEVENTF_SCANCODE` -- com `wVk = 0`, o Windows
//! resolve o scancode pelo layout da janela em foco. E o que mantem a
//! promessa: cada maquina aplica o PROPRIO layout, como um teclado de
//! verdade ligado nela. Mandar tecla virtual faria o layout do Mac
//! decidir o que aparece no Windows.
//!
//! A tabela em si fica em `input::scancode_de_evdev`, junto do seu
//! inverso e dos testes que a cobrem.
//!
//! ## Mouse vai por posicao normalizada do DESKTOP VIRTUAL
//!
//! O cliente manda pixel no espaco do que foi capturado -- UM monitor.
//! O `SendInput` absoluto quer 0..65535 sobre o desktop virtual INTEIRO.
//! Sem converter, em maquina de dois monitores o cursor iria para o
//! lugar errado por um fator igual a razao das larguras.

#![cfg(windows)]

use crate::input::{scancode_de_evdev, Evento, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VK_PAUSE, VIRTUAL_KEY,
};
use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

/// Um passo de roda. O cliente manda deslocamento continuo em "pixels"
/// (linha = 15 px, ver o visualizador), entao a conversao e
/// `px / 15 * 120` -- com o resto guardado, senao rolagem de trackpad
/// (fracao de linha por evento) nunca passaria do zero.
const WHEEL_DELTA: f64 = 120.0;
const PX_POR_LINHA: f64 = 15.0;

/// Aplica os eventos do cliente nesta maquina.
///
/// Guarda estado (resto da rolagem e a origem do monitor capturado), por
/// isso e struct e nao funcao solta.
pub struct Teclado {
    /// Canto do monitor capturado dentro do desktop virtual.
    ///
    /// Compartilhado e nao copiado: se a pessoa rearranjar os monitores
    /// durante a sessao, a captura atualiza isto e o cursor continua
    /// caindo no lugar certo. Uma copia ficaria velha em silencio, que e
    /// o pior jeito de este bug aparecer.
    origem: std::sync::Arc<std::sync::Mutex<(i32, i32)>>,
    resto_x: f64,
    resto_y: f64,
}

impl Teclado {
    pub fn novo(origem: std::sync::Arc<std::sync::Mutex<(i32, i32)>>) -> Self {
        Teclado { origem, resto_x: 0.0, resto_y: 0.0 }
    }

    pub fn aplicar(&mut self, e: Evento) {
        match e {
            Evento::Mouse { x, y } => self.mover(x, y),
            Evento::MouseRel { dx, dy } => self.mover_relativo(dx, dy),
            Evento::Botao { codigo, apertado } => self.botao(codigo, apertado),
            Evento::Rolagem { dx, dy } => self.rolar(dx, dy),
            Evento::Tecla { keycode, apertado } => self.tecla(keycode, apertado),
        }
    }

    fn mover(&self, x: f64, y: f64) {
        let (vx, vy, vw, vh) = virtual_screen();
        if vw <= 1 || vh <= 1 {
            return;
        }
        let Ok(origem) = self.origem.lock() else { return };
        // Pixel do stream -> pixel do desktop -> 0..65535 do virtual.
        let dx = origem.0 as f64 + x;
        let dy = origem.1 as f64 + y;
        let nx = ((dx - vx as f64) * 65535.0 / (vw - 1) as f64).round();
        let ny = ((dy - vy as f64) * 65535.0 / (vh - 1) as f64).round();
        envia(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: nx.clamp(0.0, 65535.0) as i32,
                    dy: ny.clamp(0.0, 65535.0) as i32,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }

    fn botao(&self, codigo: i32, apertado: bool) {
        let flag: MOUSE_EVENT_FLAGS = match (codigo, apertado) {
            (BTN_LEFT, true) => MOUSEEVENTF_LEFTDOWN,
            (BTN_LEFT, false) => MOUSEEVENTF_LEFTUP,
            (BTN_RIGHT, true) => MOUSEEVENTF_RIGHTDOWN,
            (BTN_RIGHT, false) => MOUSEEVENTF_RIGHTUP,
            (BTN_MIDDLE, true) => MOUSEEVENTF_MIDDLEDOWN,
            (BTN_MIDDLE, false) => MOUSEEVENTF_MIDDLEUP,
            _ => return,
        };
        envia(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flag,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }

    fn rolar(&mut self, dx: f64, dy: f64) {
        // Sinal invertido: no evdev `dy` positivo e rolar PARA BAIXO; no
        // Windows a roda positiva e para CIMA.
        self.resto_y += -dy / PX_POR_LINHA * WHEEL_DELTA;
        self.resto_x += -dx / PX_POR_LINHA * WHEEL_DELTA;
        let passos_y = (self.resto_y / WHEEL_DELTA).trunc();
        if passos_y != 0.0 {
            self.resto_y -= passos_y * WHEEL_DELTA;
            let d = (passos_y * WHEEL_DELTA) as i32;
            envia(&[INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: d as u32,
                        dwFlags: MOUSEEVENTF_WHEEL,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }]);
        }
        // Rolagem horizontal fica de fora por enquanto: exige
        // MOUSEEVENTF_HWHEEL e poucas janelas a honram.
        self.resto_x = self.resto_x.clamp(-WHEEL_DELTA, WHEEL_DELTA);
    }

    fn tecla(&self, keycode: u32, apertado: bool) {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        let (vk, scan) = match scancode_de_evdev(keycode) {
            Some((scan, estendida)) => {
                flags |= KEYEVENTF_SCANCODE;
                if estendida {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                (VIRTUAL_KEY(0), scan)
            }
            // Pause nao tem scancode de um byte; vai por tecla virtual.
            None if keycode == 119 => (VK_PAUSE, 0),
            None => return,
        };
        if !apertado {
            flags |= KEYEVENTF_KEYUP;
        }
        envia(&[INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }
}

fn envia(eventos: &[INPUT]) {
    let n = unsafe { SendInput(eventos, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != eventos.len() {
        // Acontece quando a janela em foco tem privilegio maior que o
        // nosso (UIPI): o SendInput e recusado inteiro, sem erro.
        eprintln!("  SendInput recusado ({n}/{}): janela elevada em foco?", eventos.len());
    }
}

/// (x, y, largura, altura) do desktop virtual -- cobre multi-monitor, ao
/// contrario de `SM_CXSCREEN`, que so ve o primario.
fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

impl Teclado {
    /// Deslocamento relativo, sem passar por coordenada de tela.
    ///
    /// O KVM manda isto. Nao da para reaproveitar `mover()`: aquele
    /// normaliza para 0..65535 sobre o desktop virtual, e converter
    /// relativo em absoluto exigiria manter um cursor virtual nosso --
    /// que divergiria do cursor real assim que qualquer outra coisa
    /// mexesse o ponteiro.
    ///
    /// Sem `MOUSEEVENTF_ABSOLUTE`, o Windows aplica a propria aceleracao
    /// de ponteiro, que e o que a pessoa espera sentir. Jogo que le
    /// entrada bruta recebe o deslocamento direto.
    pub fn mover_relativo(&self, dx: f64, dy: f64) {
        let mut e = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: dx.round() as i32,
                    dy: dy.round() as i32,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe { SendInput(std::slice::from_mut(&mut e), std::mem::size_of::<INPUT>() as i32) };
    }
}

// --- tela apagada ----------------------------------------------------
//
// O host nao subia quando a maquina estava com a tela apagada: o
// `DuplicateOutput` recusava as cinco tentativas e o processo morria com
// a mensagem sobre tela bloqueada -- que estava errada. Nao havia
// bloqueio: mover o mouse na maquina nao pedia senha. Era so a economia
// de energia tendo desligado a saida de video, e sem saida ligada nao ha
// o que duplicar.
//
// Isso so aparece no uso real, porque e justamente quando a maquina
// ficou sozinha um tempo que voce quer alcanca-la de longe.

/// Acorda a tela sem apertar nada e sem deslocar o ponteiro.
///
/// Dois `MOUSEEVENTF_MOVE` relativos na MESMA chamada: +1 e depois -1.
/// O ponteiro acaba onde estava, e o par e atomico, entao nao da para
/// alguem clicar entre os dois.
///
/// Tem de ser movimento de verdade. A primeira versao mandava um unico
/// (0,0) achando que o contador de ociosidade bastava; o Windows
/// descarta o no-op e a tela ficou apagada. Isso apareceu medindo: o
/// `DuplicateOutput` passava, o cliente conectava, e nenhum quadro saia
/// -- porque a saida de video continuava desligada.
///
/// NAO resolve sessao realmente bloqueada: ali o desktop seguro esta na
/// frente e o DXGI nao o captura por desenho. Limite conhecido.
pub fn acordar_tela() {
    let passo = |dx: i32| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let mut par = [passo(1), passo(-1)];
    unsafe { SendInput(&mut par, std::mem::size_of::<INPUT>() as i32) };
}

/// Enquanto existir, a tela nao apaga sozinha.
///
/// Economia de energia desligando a saida no meio de uma sessao derruba
/// a duplicacao (`DXGI_ERROR_ACCESS_LOST`), entao nao basta acordar na
/// partida: e preciso segurar enquanto estamos servindo. No `Drop` o
/// estado volta ao normal, e o Windows retoma a contagem de ociosidade
/// de onde estava -- nao prendemos a maquina acordada depois que o host
/// sai.
pub struct TelaAcesa;

impl TelaAcesa {
    pub fn nova() -> Self {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
        };
        TelaAcesa
    }
}

impl Drop for TelaAcesa {
    fn drop(&mut self) {
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }
}
