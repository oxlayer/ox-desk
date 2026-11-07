//! Aplicar input no Windows: `SendInput`.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Tecla vai por SCANCODE, nao por tecla virtual
//!
//! O protocolo carrega keycode do evdev, que e a POSICAO fisica da tecla
//! (ver `input::evdev_from_winit`). Aqui ela e traduzida para scancode do
//! Set 1 e injetada com `KEYEVENTF_SCANCODE` -- com `wVk = 0`, o Windows
//! resolve o scancode pelo layout da janela em foco. E o que mantem a
//! promessa: cada maquina aplica o PROPRIO layout, como um teclado de
//! verdade ligado nela. Mandar tecla virtual faria o layout do Mac
//! decidir o que aparece no Windows.
//!
//! A tabela em si fica em `input::scancode_from_evdev`, junto do seu
//! inverso e dos testes que a cobrem.
//!
//! ## Mouse vai por posicao normalizada do DESKTOP VIRTUAL
//!
//! O cliente manda pixel no espaco do que foi capturado -- UM monitor.
//! O `SendInput` absoluto quer 0..65535 sobre o desktop virtual INTEIRO.
//! Sem converter, em maquina de dois monitores o cursor iria para o
//! lugar errado por um fator igual a razao das larguras.

#![cfg(windows)]

use crate::input::{scancode_from_evdev, Event, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
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
pub struct Injector {
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

impl Injector {
    pub fn new(origem: std::sync::Arc<std::sync::Mutex<(i32, i32)>>) -> Self {
        Injector { origem, resto_x: 0.0, resto_y: 0.0 }
    }

    pub fn apply(&mut self, e: Event) {
        match e {
            Event::Mouse { x, y } => self.mover(x, y),
            Event::MouseRel { dx, dy } => self.move_relative(dx, dy),
            Event::Button { code, pressed } => self.botao(code, pressed),
            Event::Scroll { dx, dy } => self.rolar(dx, dy),
            Event::Key { keycode, pressed } => self.tecla(keycode, pressed),
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

    fn botao(&self, code: i32, pressed: bool) {
        let flag: MOUSE_EVENT_FLAGS = match (code, pressed) {
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
        // Scroll horizontal fica de fora por enquanto: exige
        // MOUSEEVENTF_HWHEEL e poucas janelas a honram.
        self.resto_x = self.resto_x.clamp(-WHEEL_DELTA, WHEEL_DELTA);
    }

    fn tecla(&self, keycode: u32, pressed: bool) {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        let (vk, scan) = match scancode_from_evdev(keycode) {
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
        if !pressed {
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

impl Injector {
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
    pub fn move_relative(&self, dx: f64, dy: f64) {
        // Por `envia()`, e nao `SendInput` na mao: aquele confere o
        // retorno e grita quando o UIPI recusa (janela elevada em
        // foco). Chamar direto, como esta funcao fazia, engolia
        // exatamente o caso que a gente precisa ver no log.
        envia(&[INPUT {
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
        }]);
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
pub fn wake_screen() {
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
pub struct ScreenAwake;

impl ScreenAwake {
    pub fn new() -> Self {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
        };
        ScreenAwake
    }
}

impl Drop for ScreenAwake {
    fn drop(&mut self) {
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }
}

/// Encerra qualquer protetor de tela rodando. Devolve quantos matou.
///
/// **Esta era a causa real do "nao consigo acordar o Windows".**
///
/// O protetor de tela roda num desktop proprio ("Screen-saver"), e o
/// DXGI recusa duplicar para um processo que nao esta no desktop de
/// input corrente: `DuplicateOutput` devolve `E_ACCESSDENIED`, o mesmo
/// code da tela bloqueada e do driver pela metade. Diagnostiquei as
/// duas causas erradas antes de olhar a lista de processos e achar o
/// `Bubbles.scr` rodando, com o `LogonUI` ausente e o adaptador de
/// video OK.
///
/// Nao da para dispensar por `SendInput`: a injecao vai para o desktop
/// da THREAD que chama, que e o `Default` -- o protetor, noutro
/// desktop, nem fica sabendo. Por isso `wake_screen()` resolve tela
/// apagada e nao resolve esta.
///
/// `SystemParametersInfo(SPI_SETSCREENSAVEACTIVE, FALSE)` tambem nao
/// serve sozinho: impede o proximo, nao encerra o que ja esta na tela.
///
/// Terminar o processo e o que sobra, e e o que as ferramentas de
/// acesso remoto fazem. O risco e baixo: um protetor de tela nao tem
/// estado para perder, e o proprio Windows o mata a qualquer movimento
/// de mouse.
pub fn end_screensaver() -> usize {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    let mut mortos = 0usize;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return 0;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let fim = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let nome = String::from_utf16_lossy(&e.szExeFile[..fim]);
                if nome.to_ascii_lowercase().ends_with(".scr") {
                    if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, e.th32ProcessID) {
                        if TerminateProcess(h, 0).is_ok() {
                            println!("  protetor de tela encerrado: {nome}");
                            mortos += 1;
                        }
                        let _ = CloseHandle(h);
                    }
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    mortos
}
