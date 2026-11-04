//! Injecao de teclado e mouse no macOS, por CoreGraphics.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Precisa de Acessibilidade, e isso nao tem contorno
//!
//! `CGEventPost` so chega a outros programas se o processo tiver
//! permissao de **Acessibilidade**. Sem ela os eventos sao aceitos e
//! descartados em silencio -- a chamada devolve sucesso e nada
//! acontece. E o mesmo modo de falha da Gravacao de Tela: o macOS nao
//! avisa, so ignora.
//!
//! Por isso o host **verifica na partida** e diz o que falta, em vez
//! de deixar o usuario descobrir movendo o mouse e nao vendo nada.
//!
//! ## FFI cru, sem crate
//!
//! Tres funcoes e dois tipos opacos. Uma crate de bindings traria
//! centenas de simbolos que nao usamos e mais uma versao para
//! acompanhar; o `winput` do lado Windows seguiu o mesmo raciocinio.

#![cfg(target_os = "macos")]

use crate::input::{tecla_mac_de_evdev, Evento, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};

type CFTypeRef = *const std::ffi::c_void;
type CGEventRef = *mut std::ffi::c_void;
type CGEventSourceRef = *mut std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

// Tipos de evento de mouse, da `CGEventType`.
const MOUSE_MOVED: u32 = 5;
const L_DOWN: u32 = 1;
const L_UP: u32 = 2;
const R_DOWN: u32 = 3;
const R_UP: u32 = 4;
const OTHER_DOWN: u32 = 25;
const OTHER_UP: u32 = 26;
const L_DRAG: u32 = 6;
const R_DRAG: u32 = 7;

// Botoes, da `CGMouseButton`.
const BTN_ESQ: u32 = 0;
const BTN_DIR: u32 = 1;
const BTN_MEIO: u32 = 2;

// Campos de `CGEventField` para o delta relativo.
const DELTA_X: u32 = 4;
const DELTA_Y: u32 = 5;

/// `kCGHIDEventTap`: entra na fila como se viesse do hardware.
const TAP_HID: u32 = 0;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        tipo: u32,
        pos: CGPoint,
        botao: u32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        tecla: u16,
        apertado: bool,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent(
        source: CGEventSourceRef,
        unidade: u32,
        eixos: u32,
        a: i32,
        b: i32,
    ) -> CGEventRef;
    fn CGEventPost(tap: u32, evento: CGEventRef);
    fn CGEventSetIntegerValueField(evento: CGEventRef, campo: u32, valor: i64);
    fn CGEventGetLocation(evento: CGEventRef) -> CGPoint;
    fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGMainDisplayID() -> u32;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(obj: CFTypeRef);
}

/// O processo pode injetar input?
///
/// Publico porque quem avisa e o host, na partida: um KVM que aceita
/// eventos e os descarta em silencio e pior que um que recusa.
pub fn tem_acessibilidade() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub struct Teclado {
    fonte: CGEventSourceRef,
    /// Ultima posicao conhecida do ponteiro.
    ///
    /// Mantida por nos porque o movimento RELATIVO precisa de um ponto
    /// de partida, e perguntar ao sistema a cada evento custaria uma
    /// chamada a mais no caminho quente. Sincronizada com o real na
    /// criacao e sempre que chega posicao absoluta.
    pos: CGPoint,
    /// Botoes apertados, para o arraste sair como arraste.
    esq: bool,
    dir: bool,
}

impl Teclado {
    pub fn novo() -> Self {
        // 1 = kCGEventSourceStateHIDSystemState: o estado dos
        // modificadores vem do teclado real, entao um Shift que o
        // usuario esteja segurando localmente nao se perde.
        let fonte = unsafe { CGEventSourceCreate(1) };
        let pos = unsafe {
            let e = CGEventCreate(std::ptr::null_mut());
            if e.is_null() {
                CGPoint { x: 0.0, y: 0.0 }
            } else {
                let p = CGEventGetLocation(e);
                CFRelease(e as CFTypeRef);
                p
            }
        };
        Teclado { fonte, pos, esq: false, dir: false }
    }

    pub fn aplicar(&mut self, e: Evento) {
        match e {
            Evento::MouseRel { dx, dy } => self.mover_relativo(dx, dy),
            Evento::Mouse { x, y } => self.mover_absoluto(x, y),
            Evento::Botao { codigo, apertado } => self.botao(codigo, apertado),
            Evento::Rolagem { dx, dy } => self.rolar(dx, dy),
            Evento::Tecla { keycode, apertado } => self.tecla(keycode, apertado),
        }
    }

    /// Posta um evento e o libera. Esquecer o release vaza por evento.
    fn postar(&self, ev: CGEventRef) {
        if ev.is_null() {
            return;
        }
        unsafe {
            CGEventPost(TAP_HID, ev);
            CFRelease(ev as CFTypeRef);
        }
    }

    fn mover_relativo(&mut self, dx: f64, dy: f64) {
        let (w, h) = unsafe {
            let d = CGMainDisplayID();
            (CGDisplayPixelsWide(d) as f64, CGDisplayPixelsHigh(d) as f64)
        };
        // Prender na tela: sem isto a posicao acumulada sai do display
        // e o ponteiro "gruda" numa borda ate o usuario mover o mesmo
        // tanto de volta -- parece travado, e nao esta.
        self.pos.x = (self.pos.x + dx).clamp(0.0, w - 1.0);
        self.pos.y = (self.pos.y + dy).clamp(0.0, h - 1.0);

        // Arrastando? O tipo do evento muda, senao o sistema entende
        // "mover com botao apertado" como mover simples e o arraste nao
        // acontece.
        let tipo = if self.esq {
            L_DRAG
        } else if self.dir {
            R_DRAG
        } else {
            MOUSE_MOVED
        };
        let ev = unsafe { CGEventCreateMouseEvent(self.fonte, tipo, self.pos, BTN_ESQ) };
        if !ev.is_null() {
            // O delta tem de ir EXPLICITO: jogo que le entrada bruta
            // olha este campo, nao a diferenca de posicao. Sem ele, a
            // mira nao se move mesmo com o ponteiro andando.
            unsafe {
                CGEventSetIntegerValueField(ev, DELTA_X, dx as i64);
                CGEventSetIntegerValueField(ev, DELTA_Y, dy as i64);
            }
        }
        self.postar(ev);
    }

    fn mover_absoluto(&mut self, x: f64, y: f64) {
        self.pos = CGPoint { x, y };
        let ev = unsafe { CGEventCreateMouseEvent(self.fonte, MOUSE_MOVED, self.pos, BTN_ESQ) };
        self.postar(ev);
    }

    fn botao(&mut self, codigo: i32, apertado: bool) {
        let (tipo, botao) = match (codigo, apertado) {
            (c, true) if c == BTN_LEFT => {
                self.esq = true;
                (L_DOWN, BTN_ESQ)
            }
            (c, false) if c == BTN_LEFT => {
                self.esq = false;
                (L_UP, BTN_ESQ)
            }
            (c, true) if c == BTN_RIGHT => {
                self.dir = true;
                (R_DOWN, BTN_DIR)
            }
            (c, false) if c == BTN_RIGHT => {
                self.dir = false;
                (R_UP, BTN_DIR)
            }
            (c, true) if c == BTN_MIDDLE => (OTHER_DOWN, BTN_MEIO),
            (c, false) if c == BTN_MIDDLE => (OTHER_UP, BTN_MEIO),
            _ => return,
        };
        let ev = unsafe { CGEventCreateMouseEvent(self.fonte, tipo, self.pos, botao) };
        self.postar(ev);
    }

    fn rolar(&self, dx: f64, dy: f64) {
        // Unidade 0 = pixel. O protocolo manda "pixels" de rolagem e o
        // macOS aceita a mesma unidade, entao nao ha conversao.
        //
        // Sinal invertido: no nosso protocolo (e no Wayland) Y positivo
        // desce; no macOS, positivo sobe.
        let ev = unsafe {
            CGEventCreateScrollWheelEvent(self.fonte, 0, 2, -dy as i32, dx as i32)
        };
        self.postar(ev);
    }

    fn tecla(&self, keycode: u32, apertado: bool) {
        // Sem traducao, nao manda nada: postar um keycode qualquer
        // digitaria uma letra ERRADA na maquina de outra pessoa, que e
        // pior que nao digitar.
        let Some(mac) = tecla_mac_de_evdev(keycode) else { return };
        let ev = unsafe { CGEventCreateKeyboardEvent(self.fonte, mac, apertado) };
        self.postar(ev);
    }
}

// A fonte de eventos vive enquanto o host viver; nao ha caminho em que
// um `Teclado` seja descartado e recriado, entao nao ha Drop. Se um dia
// houver, ele precisa de `CFRelease(self.fonte)`.

/// Thread dona do `Teclado`, alimentada por canal.
///
/// Os tipos do CoreGraphics sao ponteiros crus e portanto `!Send`, e a
/// tarefa de rede do host e `tokio::spawn` -- que exige `Send`. Daria
/// para declarar `unsafe impl Send` apostando que `CGEventPost` e
/// thread-safe, mas e uma afirmacao que nao da para verificar daqui.
///
/// Uma thread dona resolve sem aposta nenhuma, e e a mesma forma que o
/// host do Linux ja usa para a conexao D-Bus (que tambem e `!Send` e
/// cuja sessao pertence a conexao que a criou). O custo e um salto de
/// canal por evento -- microssegundos, contra ~300 ms de rede.
pub fn thread() -> std::sync::mpsc::Sender<Evento> {
    let (tx, rx) = std::sync::mpsc::channel::<Evento>();
    std::thread::spawn(move || {
        let mut t = Teclado::novo();
        while let Ok(e) = rx.recv() {
            t.aplicar(e);
        }
    });
    tx
}
