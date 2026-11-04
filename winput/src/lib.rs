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

// --- clipboard -------------------------------------------------------
//
// Existe para o `logid` nao precisar chamar `powershell -Command
// Get-Clipboard` a cada amostragem. Aquilo funcionava, mas spawnar um
// PowerShell a cada 2 s custa ~200 ms de CPU por leitura -- e, se quem
// chama esquecer `CREATE_NO_WINDOW`, abre uma janela de console
// visivel a cada volta. Foi o que aconteceu: "o windows ta abrindo um
// powershell a cada 5s".
//
// Texto Unicode apenas (CF_UNICODETEXT). Arquivos sao CF_HDROP e ficam
// para quando o clipboard de arquivos existir.

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(h: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn SetClipboardData(format: u32, mem: isize) -> isize;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
    fn GlobalLock(mem: isize) -> *mut u16;
    fn GlobalUnlock(mem: isize) -> i32;
    fn GlobalFree(mem: isize) -> isize;
}

/// Abre o clipboard, roda `f`, e fecha mesmo se `f` falhar.
///
/// Fechar e obrigatorio: o clipboard e um recurso GLOBAL, e deixa-lo
/// aberto trava o Ctrl+C e o Ctrl+V de todos os programas da sessao
/// ate o processo morrer. Por isso nao ha caminho de saida que pule o
/// `CloseClipboard`.
fn com_clipboard<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    // O clipboard pode estar tomado por outro programa por alguns ms.
    // Cinco tentativas com 20 ms cobrem o caso normal sem travar o
    // chamador quando alguem o segura de verdade.
    let mut aberto = false;
    for _ in 0..5 {
        if unsafe { OpenClipboard(0) } != 0 {
            aberto = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !aberto {
        return Err("o clipboard esta ocupado por outro programa".into());
    }
    let r = f();
    unsafe { CloseClipboard() };
    r
}

/// Texto do clipboard, ou `None` se nao houver texto nele.
pub fn clipboard_ler() -> Result<Option<String>, String> {
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return Ok(None);
    }
    com_clipboard(|| {
        let mem = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if mem == 0 {
            return Ok(None);
        }
        let p = unsafe { GlobalLock(mem) };
        if p.is_null() {
            return Err("GlobalLock falhou na leitura".into());
        }
        // UTF-16 terminado em zero. Andar ate o terminador e o unico
        // jeito: o tamanho do bloco e arredondado e pode sobrar lixo.
        let mut n = 0usize;
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) });
        unsafe { GlobalUnlock(mem) };
        Ok(Some(s))
    })
}

/// Poe texto no clipboard.
pub fn clipboard_escrever(texto: &str) -> Result<(), String> {
    let mut u: Vec<u16> = texto.encode_utf16().collect();
    u.push(0);
    let bytes = u.len() * 2;

    com_clipboard(|| {
        let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if mem == 0 {
            return Err("GlobalAlloc falhou".into());
        }
        let p = unsafe { GlobalLock(mem) };
        if p.is_null() {
            unsafe { GlobalFree(mem) };
            return Err("GlobalLock falhou na escrita".into());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(u.as_ptr(), p, u.len());
            GlobalUnlock(mem);
        }
        unsafe { EmptyClipboard() };
        // Depois do SetClipboardData o bloco pertence ao SISTEMA --
        // liberar aqui seria use-after-free no proximo Ctrl+V. So
        // liberamos quando o Set FALHA e a posse nao mudou de mao.
        if unsafe { SetClipboardData(CF_UNICODETEXT, mem) } == 0 {
            unsafe { GlobalFree(mem) };
            return Err("SetClipboardData falhou".into());
        }
        Ok(())
    })
}

// --- clipboard de arquivos -------------------------------------------
//
// Arquivos no clipboard do Windows sao `CF_HDROP`: um `DROPFILES`
// seguido de uma lista de caminhos UTF-16, cada um terminado em zero e
// a lista inteira terminada por um zero extra. Nao e texto, e nao da
// para reaproveitar o caminho de `CF_UNICODETEXT`.

const CF_HDROP: u32 = 15;

#[repr(C)]
struct DropFiles {
    /// Offset do inicio da lista de nomes, a partir deste struct.
    p_files: u32,
    pt_x: i32,
    pt_y: i32,
    /// Coordenadas sao de area nao-cliente? Nao usamos; fica 0.
    f_nc: i32,
    /// Lista em UTF-16? Sempre 1 aqui -- a versao ANSI perderia acento.
    f_wide: i32,
}

#[link(name = "shell32")]
extern "system" {
    fn DragQueryFileW(drop: isize, indice: u32, buf: *mut u16, tam: u32) -> u32;
}

/// Caminhos de arquivo no clipboard, ou `None` se nao houver arquivos.
pub fn clipboard_ler_arquivos() -> Result<Option<Vec<String>>, String> {
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) } == 0 {
        return Ok(None);
    }
    com_clipboard(|| {
        let h = unsafe { GetClipboardData(CF_HDROP) };
        if h == 0 {
            return Ok(None);
        }
        // 0xFFFF_FFFF pede a CONTAGEM em vez de um nome.
        let n = unsafe { DragQueryFileW(h, 0xFFFF_FFFF, std::ptr::null_mut(), 0) };
        let mut v = Vec::with_capacity(n as usize);
        for i in 0..n {
            // Primeiro o tamanho, depois o nome: pedir com buffer curto
            // trunca em silencio, e caminho truncado aponta para outro
            // arquivo -- ou para nenhum.
            let tam = unsafe { DragQueryFileW(h, i, std::ptr::null_mut(), 0) };
            if tam == 0 {
                continue;
            }
            let mut buf = vec![0u16; tam as usize + 1];
            let lidos = unsafe { DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32) };
            if lidos == 0 {
                continue;
            }
            v.push(String::from_utf16_lossy(&buf[..lidos as usize]));
        }
        Ok(if v.is_empty() { None } else { Some(v) })
    })
}

/// Poe uma lista de arquivos no clipboard.
pub fn clipboard_escrever_arquivos(caminhos: &[String]) -> Result<(), String> {
    if caminhos.is_empty() {
        return Err("lista de arquivos vazia".into());
    }
    // DROPFILES + nomes UTF-16, cada um com \0, mais um \0 final.
    let mut nomes: Vec<u16> = Vec::new();
    for c in caminhos {
        nomes.extend(c.encode_utf16());
        nomes.push(0);
    }
    nomes.push(0);

    let cab = std::mem::size_of::<DropFiles>();
    let bytes = cab + nomes.len() * 2;

    com_clipboard(|| {
        let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if mem == 0 {
            return Err("GlobalAlloc falhou".into());
        }
        let p = unsafe { GlobalLock(mem) } as *mut u8;
        if p.is_null() {
            unsafe { GlobalFree(mem) };
            return Err("GlobalLock falhou".into());
        }
        unsafe {
            let df = p as *mut DropFiles;
            std::ptr::write(
                df,
                DropFiles { p_files: cab as u32, pt_x: 0, pt_y: 0, f_nc: 0, f_wide: 1 },
            );
            std::ptr::copy_nonoverlapping(nomes.as_ptr(), p.add(cab) as *mut u16, nomes.len());
            GlobalUnlock(mem);
            EmptyClipboard();
        }
        // Mesma regra do texto: depois do Set o bloco e do SISTEMA.
        if unsafe { SetClipboardData(CF_HDROP, mem) } == 0 {
            unsafe { GlobalFree(mem) };
            return Err("SetClipboardData(CF_HDROP) falhou".into());
        }
        Ok(())
    })
}
