//! O que o Windows deixa um processo SYSTEM fazer na tela de bloqueio.
//!
//! Spec: ../../../specs/014-lock-screen.md
//!
//! Mesmo espírito do `mac-probe`: provar as peças antes de montar o
//! pipeline. Aqui a pergunta que decide o desenho é uma só e **varia
//! por versão do Windows** -- há relatos de o processo morrer logo
//! depois do bloqueio no Win 11 e relatos de funcionar bem como
//! SYSTEM. Não dá para decidir por leitura.
//!
//!   logistream-probe-sistema --quem        quem sou eu, e em que mesa
//!   logistream-probe-sistema --capturar    consigo duplicar a tela daqui?
//!   logistream-probe-sistema --lancar      vira SYSTEM na mesa Winlogon
//!
//! ## O caminho que estamos provando
//!
//! A tela de bloqueio e o diálogo de UAC vivem numa **mesa** (desktop)
//! própria, `Winlogon`, dentro da window station `WinSta0`. O DXGI não
//! duplica mesa que não seja a de entrada, e só processo LocalSystem
//! consegue se anexar à `Winlogon`. Um processo de usuário -- que é o
//! que somos hoje, via tarefa `/IT` -- não tem como nem olhar.
//!
//! O padrão que Parsec e RustDesk usam, e que `--lancar` exercita:
//!
//! 1. achar o `winlogon.exe` da sessão do console (`WTSGetActiveConsoleSessionId`)
//! 2. abrir o token dele e duplicar como token primário
//! 3. `CreateProcessAsUserW` com `lpDesktop = "winsta0\\Winlogon"`
//!
//! O filho nasce SYSTEM, na sessão do console, já na mesa certa.
//!
//! ## Por que um probe e não já o serviço
//!
//! Instalar serviço é mexer em `HKLM` e criar um processo SYSTEM
//! permanente -- e o Defender já apagou o `logid.exe` desta máquina uma
//! vez (spec 008). Antes de pedir isso, vale saber se o caminho
//! funciona: `--lancar` consegue o mesmo efeito a partir de uma tarefa
//! agendada `/ru SYSTEM`, que o administrador cria e apaga sem deixar
//! resíduo.


#[cfg(windows)]
mod win {
use std::ffi::c_void;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, LookupPrivilegeValueW, SecurityImpersonation,
    TokenPrimary, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop, UOI_NAME,
    DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetCurrentProcessId,
    OpenProcessToken, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};
use windows::Win32::System::Threading::OpenProcess;

/// Imprime e **grava**.
///
/// O processo lançado por `--lancar` nasce sem console (de propósito:
/// console novo abriria uma janela preta na tela de bloqueio). Sem
/// arquivo, a saída dele não iria a lugar nenhum -- e a saída dele é o
/// resultado inteiro do experimento.
macro_rules! diga {
    ($($a:tt)*) => {{
        let linha = format!($($a)*);
        println!("{linha}");
        let _ = std::fs::create_dir_all("C:\\ProgramData\\logid");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true).append(true).open("C:\\ProgramData\\logid\\probe-sistema.log")
        {
            use std::io::Write;
            let _ = writeln!(f, "{linha}");
        }
    }};
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Nome da mesa (desktop) a que esta thread está anexada.
fn mesa_atual() -> String {
    unsafe {
        let h = windows::Win32::System::StationsAndDesktops::GetThreadDesktop(
            windows::Win32::System::Threading::GetCurrentThreadId(),
        );
        let Ok(h) = h else { return "?".into() };
        nome_do_objeto(h.0 as *mut c_void)
    }
}

fn nome_do_objeto(h: *mut c_void) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let mut n = 0u32;
        let ok = GetUserObjectInformationW(
            windows::Win32::Foundation::HANDLE(h),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut c_void),
            (buf.len() * 2) as u32,
            Some(&mut n),
        );
        if ok.is_err() {
            return "?".into();
        }
        let fim = buf.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..fim])
    }
}

fn sou_system() -> bool {
    // Jeito barato e sem SID: SYSTEM tem USERNAME=<maquina>$ ou
    // SYSTEM dependendo do caminho. O confiavel aqui e o nome de
    // usuario do token, mas para um probe o que importa e so
    // distinguir "sou eu" de "sou SYSTEM", e o USERNAME basta.
    std::env::var("USERNAME").map(|u| u.eq_ignore_ascii_case("SYSTEM") || u.ends_with('$'))
        .unwrap_or(false)
}

fn sessao_desta() -> u32 {
    unsafe {
        let mut s = 0u32;
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut s);
        s
    }
}

fn quem() {
    diga!("  pid {} sessão {}", unsafe { GetCurrentProcessId() }, sessao_desta());
    diga!("  sessão do console: {}", unsafe { WTSGetActiveConsoleSessionId() });
    diga!("  USERNAME={} (SYSTEM? {})", std::env::var("USERNAME").unwrap_or_default(), sou_system());
    diga!("  mesa desta thread: {}", mesa_atual());
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0)) {
            Ok(h) => {
                // A PERGUNTA. Mesa de entrada "Winlogon" = tela
                // bloqueada ou UAC na frente. "Default" = area de
                // trabalho normal. Processo de usuario nao consegue
                // abrir a Winlogon de jeito nenhum -- o proprio erro
                // aqui ja e resposta.
                diga!("  mesa de ENTRADA: {}", nome_do_objeto(h.0 as *mut c_void));
                let _ = CloseDesktop(h);
            }
            Err(e) => diga!("  mesa de ENTRADA: não consegui abrir ({e}) -- típico de processo de usuário"),
        }
    }
}

/// Anexa esta thread à mesa de entrada e tenta duplicar a saída.
fn capturar() {
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0)) {
            Ok(h) => {
                let nome = nome_do_objeto(h.0 as *mut c_void);
                match SetThreadDesktop(h) {
                    Ok(()) => diga!("  anexado à mesa de entrada: {nome}"),
                    Err(e) => diga!("  NÃO anexei a {nome}: {e}"),
                }
            }
            Err(e) => diga!("  não abri a mesa de entrada: {e}"),
        }
    }
    diga!("  mesa desta thread agora: {}", mesa_atual());

    // O teste real: o mesmo caminho do host de verdade.
    match logistream::capture_win::Monitor::abrir(0) {
        Ok(m) => match m.duplicar() {
            Ok(t) => {
                let (lw, lh) = t.tamanho();
                diga!("  DUPLICOU: {lw}x{lh} -- captura funciona nesta mesa");
            }
            Err(e) => diga!("  duplicar FALHOU: {e}"),
        },
        Err(e) => diga!("  abrir monitor falhou: {e}"),
    }
}

/// Acompanha a mesa de entrada por N segundos, capturando a cada volta.
///
/// Existe porque a primeira medição se contradisse: o `LogonUI` estava
/// na tela (ou seja, bloqueado) e o probe leu a mesa de entrada como
/// `Default`. Uma foto só não distingue "a leitura está errada" de "o
/// bloqueio ainda não tinha trocado a mesa quando eu olhei". Um filme
/// distingue -- e de quebra mostra QUANTO tempo a troca leva, que é
/// exatamente o que o serviço vai precisar tolerar.
#[cfg(windows)]
fn seguir(segundos: u64) {
    for i in 0..segundos {
        let (mesa, dup) = unsafe {
            match OpenInputDesktop(
                DESKTOP_CONTROL_FLAGS(0),
                false,
                DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0),
            ) {
                Ok(h) => {
                    let nome = nome_do_objeto(h.0 as *mut c_void);
                    // Reanexar A CADA VOLTA: a duplicação do DXGI é
                    // por mesa, e quem não reanexa continua olhando a
                    // mesa antiga -- que é como um capturador "funciona"
                    // e entrega quadro velho.
                    let _ = SetThreadDesktop(h);
                    let r = match logistream::capture_win::Monitor::abrir(0) {
                        Ok(m) => match m.duplicar() {
                            Ok(t) => {
                                let (w, h2) = t.tamanho();
                                format!("duplicou {w}x{h2}")
                            }
                            Err(e) => format!("NAO duplicou: {}", primeira_linha(&e.to_string())),
                        },
                        Err(e) => format!("sem monitor: {}", primeira_linha(&e.to_string())),
                    };
                    (nome, r)
                }
                Err(e) => ("<nao abri>".to_string(), format!("OpenInputDesktop: {e}")),
            }
        };
        diga!("  [{i:02}s] mesa de entrada={mesa:12} {dup}");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(windows)]
fn primeira_linha(s: &str) -> String {
    s.lines().next().unwrap_or(s).chars().take(90).collect()
}

/// Acha um processo pelo nome e devolve o pid na sessão pedida.
fn pid_de(nome: &str, sessao: u32) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut achou = None;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let fim = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
                let exe = String::from_utf16_lossy(&e.szExeFile[..fim]);
                if exe.eq_ignore_ascii_case(nome) {
                    let mut s = 0u32;
                    if ProcessIdToSessionId(e.th32ProcessID, &mut s).is_ok() && s == sessao {
                        achou = Some(e.th32ProcessID);
                        break;
                    }
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        achou
    }
}

/// Liga um privilégio no nosso próprio token.
fn ligar_privilegio(nome: &str) -> bool {
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut luid = LUID::default();
        let n = w(nome);
        let ok = LookupPrivilegeValueW(None, PWSTR(n.as_ptr() as *mut u16), &mut luid).is_ok();
        if ok {
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            let _ = AdjustTokenPrivileges(tok, false, Some(&tp), 0, None, None);
        }
        let _ = CloseHandle(tok);
        ok
    }
}

/// O "service dance": virar SYSTEM na mesa Winlogon da sessão do console.
fn lancar(resto: &[String]) {
    let sessao = unsafe { WTSGetActiveConsoleSessionId() };
    diga!("  sessão do console: {sessao}");

    // `winlogon.exe` é o alvo porque ele SEMPRE existe na sessão
    // interativa, roda como SYSTEM, e existe mesmo sem usuário logado
    // -- que é exatamente o estado da tela de bloqueio. O `explorer`
    // não serve: ele é do usuário e some quando ninguém está logado.
    let Some(pid) = pid_de("winlogon.exe", sessao) else {
        diga!("  não achei winlogon.exe na sessão {sessao}");
        return;
    };
    diga!("  winlogon.exe pid {pid}");

    // Sem estes dois, o CreateProcessAsUser falha com ERROR_PRIVILEGE_NOT_HELD
    // e a mensagem não menciona privilégio nenhum.
    for p in ["SeDebugPrivilege", "SeIncreaseQuotaPrivilege", "SeAssignPrimaryTokenPrivilege"] {
        diga!("  privilégio {p}: {}", if ligar_privilegio(p) { "ligado" } else { "NÃO" });
    }

    unsafe {
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            diga!("  não abri o processo winlogon (preciso ser SYSTEM ou ter SeDebugPrivilege)");
            return;
        };
        let mut tok = HANDLE::default();
        if OpenProcessToken(proc, TOKEN_DUPLICATE | TOKEN_QUERY, &mut tok).is_err() {
            diga!("  não abri o token do winlogon");
            let _ = CloseHandle(proc);
            return;
        }
        let mut primario = HANDLE::default();
        // Token PRIMÁRIO, não de impersonação: o CreateProcessAsUser
        // recusa o de impersonação, e a documentação é explícita --
        // "o token criado por DuplicateToken não pode ser usado em
        // CreateProcessAsUser".
        if DuplicateTokenEx(tok, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut primario).is_err() {
            diga!("  DuplicateTokenEx falhou");
            let _ = CloseHandle(tok);
            let _ = CloseHandle(proc);
            return;
        }

        let eu = std::env::current_exe().unwrap_or_default();
        let args: Vec<String> = resto.to_vec();
        let linha = format!("\"{}\" {}", eu.display(), args.join(" "));
        let mut linha_w = w(&linha);
        // `winsta0\Winlogon` é o ponto inteiro: é a mesa do bloqueio e
        // do UAC. `winsta0\default` daria a área de trabalho normal.
        let mut mesa = w("winsta0\\Winlogon");

        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(mesa.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let r = CreateProcessAsUserW(
            Some(primario),
            None,
            Some(PWSTR(linha_w.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            None,
            None,
            &si,
            &mut pi,
        );
        match r {
            Ok(()) => diga!("  LANCEI pid {} na mesa winsta0\\Winlogon", pi.dwProcessId),
            Err(e) => diga!("  CreateProcessAsUser falhou: {e}"),
        }
        let _ = CloseHandle(primario);
        let _ = CloseHandle(tok);
        let _ = CloseHandle(proc);
    }
}

pub fn executar() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let modo = a.first().map(String::as_str).unwrap_or("--quem");
    diga!("== logistream-probe-sistema {modo} ==");
    // Carimbo de hora: este probe roda por tarefa agendada, sem
    // console, e sem a hora não dá para saber se a saída é desta
    // execução ou da anterior.
    diga!("  {}", chrono_simples());
    match modo {
        "--quem" => quem(),
        "--capturar" => {
            quem();
            capturar();
        }
        "--lancar" => lancar(if a.len() > 1 { &a[1..] } else { &[] }),
        "--seguir" => {
            quem();
            let n = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(15);
            seguir(n);
        }
        outro => diga!("  modo desconhecido: {outro}"),
    }
}

/// Hora local sem puxar dependência: o probe não merece uma crate.
fn chrono_simples() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}
}

#[cfg(windows)]
fn main() {
    win::executar();
}

// Mesmo padrao do mac-probe: no SO errado o binario existe e diz o que
// e, em vez de nao compilar. O deploy constroi TODOS os bins do crate
// no Ubuntu, entao um bin que so compila no Windows quebraria o deploy
// inteiro -- foi o que aconteceu.
#[cfg(not(windows))]
fn main() {
    eprintln!("este probe e do Windows: ele mede o que um processo SYSTEM alcanca na tela de bloqueio");
}
