#![cfg(windows)]
//! Win32 que o `logid` e o `logid-agent` compartilham.
//!
//! Estava dentro do agente como `input.rs`. Virou crate quando o daemon
//! passou a rodar no Windows tambem: duas copias de FFI `unsafe` que
//! precisam concordar sobre sessao e instancia unica e exatamente o tipo
//! de duplicacao que silencia um bug em um dos lados.
//!
//! Idade do ultimo input na sessao interativa do Windows.
//!
//! E o sinal do dead-man's switch (spec 005): depois de receber o
//! dispositivo, se nenhum input chegar em N segundos, o agente devolve o
//! canal por conta propria -- porque a maquina de ORIGEM nao pode desfazer.
//!
//! GetLastInputInfo e por SESSAO: um servico na sessao 0 nao ve o input do
//! usuario. Por isso o guard tem que rodar na sessao interativa (atalho em
//! shell:startup), nao como servico.

#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetTickCount64() -> u64;
}

/// Milissegundos desde o ultimo input (teclado ou mouse) nesta sessao.
pub fn idle_ms() -> Result<u64, String> {
    let mut lii = LastInputInfo {
        cb_size: std::mem::size_of::<LastInputInfo>() as u32,
        dw_time: 0,
    };
    let ok = unsafe { GetLastInputInfo(&mut lii) };
    if ok == 0 {
        return Err("GetLastInputInfo falhou".into());
    }
    let now = unsafe { GetTickCount64() };
    // dw_time e 32-bit e da volta a cada ~49 dias; GetTickCount64 nao.
    // Truncar o agora para 32 bits e subtrair com wrapping da a diferenca
    // correta mesmo na virada.
    Ok((now as u32).wrapping_sub(lii.dw_time) as u64)
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[link(name = "user32")]
extern "system" {
    fn GetCursorPos(p: *mut Point) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

// Area virtual: cobre multi-monitor, ao contrario de SM_CXSCREEN que so
// reporta o monitor primario.
const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;

pub fn cursor() -> Option<Point> {
    let mut p = Point::default();
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    Some(p)
}

/// (x, y, largura, altura) da area virtual.
pub fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// O cursor esta encostado na borda `edge`?
///
/// No Windows nao precisamos de portal nem de barreira do compositor: ler
/// GetCursorPos resolve, e e por isso que este lado e mais simples que o do
/// GNOME. Tolerancia de 1 px porque o cursor para na ultima coluna valida.
pub fn at_edge(edge: &str, p: Point) -> bool {
    let (x, y, w, h) = virtual_screen();
    match edge {
        "left" => p.x <= x,
        "right" => p.x >= x + w - 1,
        "top" => p.y <= y,
        "bottom" => p.y >= y + h - 1,
        _ => false,
    }
}


#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *const u8, initial_owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
    fn WTSGetActiveConsoleSessionId() -> u32;
    fn ProcessIdToSessionId(pid: u32, sid: *mut u32) -> i32;
    fn GetCurrentProcessId() -> u32;
}

const ERROR_ALREADY_EXISTS: u32 = 183;

/// Garante uma instancia so dos modos residentes.
///
/// Sem isto, o atalho do Startup mais um lancamento manual empilham: ja
/// aconteceu -- tres `watch-edge` ao mesmo tempo, competindo pelo mesmo
/// receptor e triplicando o trafego HID++.
///
/// O handle e deliberadamente vazado: o mutex tem de viver enquanto o
/// processo viver, e o Windows o libera no encerramento.
/// Estamos na sessao do console fisico?
///
/// Os modos residentes SO funcionam la: GetLastInputInfo e GetCursorPos
/// sao por sessao. Rodar na sessao 0 (o caso de lancar por SSH) nao e
/// inofensivo -- aquela instancia nao ve input nenhum, reporta um desktop
/// falso de 1024x768 e um idle igual ao uptime, e ainda compete pelo
/// receptor HID++ com a instancia de verdade.
pub fn in_console_session() -> bool {
    let console = unsafe { WTSGetActiveConsoleSessionId() };
    let mut mine: u32 = u32::MAX;
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut mine) };
    if ok == 0 || console == u32::MAX {
        // Nao deu para saber: seguir e melhor que recusar.
        return true;
    }
    mine == console
}

pub fn claim_single_instance(name: &str) -> bool {
    // Local\ e nao Global\: o Global exige SeCreateGlobalPrivilege, e
    // quando a criacao falha nao da para distinguir "ja existe" de "nao
    // pude criar" -- medido, duas instancias passaram. O caso entre
    // sessoes ja e barrado por `in_console_session`, entao o namespace por
    // sessao e exatamente o escopo que falta cobrir.
    let wide: Vec<u16> = format!("Local\\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if h == 0 {
        // Nao conseguimos criar o mutex: seguir e melhor que nao rodar.
        return true;
    }
    unsafe { GetLastError() != ERROR_ALREADY_EXISTS }
}
