//! Setup measurement and repair, on the MacBook.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! The most treacherous part of the set lives here, for one reason only:
//! on macOS a missing permission almost never looks like a missing
//! permission. AVFoundation **hangs** without Screen Recording,
//! `codesign` fails with `errSecInternalComponent` without the keychain
//! unlocked, and input injection is accepted and dropped without
//! Accessibility. Three symptoms that do not mention the cause.
//!
//! ## Why the permissions are asked of the HOST, from outside
//!
//! TCC ties the grant to the binary -- to its designated requirement.
//! Asking from here, from the `logid` process, would give a correct
//! answer about the wrong program. So the measurement runs
//! `logistream-host --conferir` and reads the two lines it prints.
//!
//! ## Order matters, and the catalog enforces it
//!
//! `gravacao-de-tela` and `acessibilidade` depend on `assinatura`. It is
//! not fussiness: with an ad-hoc binary the binding is by cdhash, which
//! changes on every build, so granting before signing **wastes** the
//! grant -- and only the machine's owner can give it back, by hand, in
//! the UI. That has already happened twice here.

use crate::config::Config;
use crate::setup::{State, Check};
use crate::setup_common::run_timed;
use std::path::PathBuf;

const BINS: [&str; 5] = ["logid", "logistream-host", "logistream-view", "logistream-ponte", "logi-ota"];
const SIGNING_IDENTITY: &str = "logi local signing";
const KEYCHAIN: &str = "Library/Keychains/logi.keychain-db";

fn bin_dir() -> PathBuf {
    crate::sys::home_dir().join(".local/bin")
}
fn host_bin() -> PathBuf {
    bin_dir().join("logistream-host")
}
fn uid() -> u32 {
    // Same path `sys::macos::unit_state` uses. `id -u` would give the
    // same number at the cost of one process per measurement.
    unsafe { getuid() }
}
extern "C" {
    fn getuid() -> u32;
}

pub fn measure(cfg: &Config) -> Vec<Check> {
    // Both permissions come from ONE run of the host: they are two lines
    // of the same output, and running the binary twice would double the
    // cost of the most expensive measurement on the list without
    // answering anything more.
    let tcc = check_permissions();
    vec![
        binaries(),
        crate::setup_common::this_machine_config(cfg),
        daemon(),
        ffmpeg(),
        identity(),
        signature(),
        tcc.0,
        tcc.1,
        launchagent(),
        crate::setup_common::clipboard(),
        remote_login(),
        tailscale(),
        crate::setup_common::peers(cfg),
        ota(),
    ]
}

fn ota_token() -> PathBuf {
    crate::sys::home_dir().join(".config/logi/ota-token")
}

fn ota() -> Check {
    crate::setup_common::ota(&bin_dir().join("logi-ota"), &ota_token(), clock())
}

/// The LaunchAgent that wakes `logi-ota` up.
///
/// `launchctl print` and not `list`: `list` answers for the label even
/// when the job is loaded from a plist that no longer exists, and here
/// the question is "will launchd wake this up?".
fn clock() -> Result<String, String> {
    let label = "ai.logi.ota";
    let output = run_timed("/bin/launchctl", &["print", &format!("gui/{}/{label}", uid())], 5)
        .map_err(|e| format!("{label}: {e}"))?;
    // The interval comes from launchd itself, not from what we wrote: a
    // hand-edited plist (or one from an earlier version) would show up as
    // today's interval if the phrase came from a constant of ours.
    let s = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("interval ="))
        .and_then(|l| l.split('=').nth(1))
        .map(|v| v.trim().to_string());
    Ok(match s {
        Some(secs) => format!("LaunchAgent a cada {secs} s"),
        None => format!("{label} carregado (o launchd não informou intervalo)"),
    })
}

fn binaries() -> Check {
    let dir = bin_dir();
    let absent: Vec<&str> = BINS.iter().copied().filter(|b| !dir.join(b).exists()).collect();
    if !absent.is_empty() {
        return Check::missing("binarios", format!("faltam: {}", absent.join(", ")));
    }
    // Existing on disk is not being live. See
    // `setup_common::stamp_verdict`: a deploy can install the new file
    // and nobody runs it.
    if let Some(p) = crate::setup_common::daemon_on_disk(&dir.join("logid")) {
        return Check::missing("binarios", p);
    }
    Check::ok("binarios", format!("{} binários em {}", BINS.len(), dir.display()))
}

fn daemon() -> Check {
    let label = "ai.logi.logid";
    let loaded = run_timed("/bin/launchctl", &["print", &format!("gui/{}/{label}", uid())], 5);
    match loaded {
        // Loaded is not running. `print` brings `state` and
        // `last exit code`; ignoring them passed a job in a crash loop --
        // see `setup_common::launchd_verdict`.
        Ok(output) => match crate::setup_common::launchd_verdict(&output) {
            Some(p) => Check::missing("daemon", format!("{label}: {p}")),
            None => Check::ok("daemon", format!("{label} rodando na sessão gráfica")),
        },
        // A live process without a LaunchAgent is the misleading case: it
        // works now and does not come back on the next login.
        Err(_) if crate::setup_common::process_alive("logid") => {
            Check::missing("daemon", "o processo está rodando, mas fora do launchd: não volta no próximo login")
        }
        Err(e) => Check::missing("daemon", format!("{label}: {e}")),
    }
}

/// Do the brew dylibs the installed binaries ask for exist here?
///
/// The ffmpeg soname goes into the binary. A `logistream-host` from the
/// CI package asks for `libavcodec.61` under the `ffmpeg@7` prefix; if
/// the formula is not installed, `dyld` refuses the process and
/// AVFoundation is never reached. Asking `otool` what the file requests,
/// and checking that each path exists, is cheap and answers exactly that.
///
/// See FFMPEG_FORMULA in scripts/construir-binarios.sh: both ends declare
/// the same prefix, and the build breaks if they diverge.
fn ffmpeg() -> Check {
    let host = bin_dir().join("logistream-host");
    if !host.exists() {
        return Check::missing("ffmpeg", "sem o logistream-host instalado para perguntar");
    }
    let output = match run_timed("/usr/bin/otool", &["-L", &host.to_string_lossy()], 10) {
        Ok(s) => s,
        Err(e) => return Check::error("ffmpeg", format!("otool: {e}")),
    };
    let requested: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("/opt/"))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    let absent: Vec<&&str> =
        requested.iter().filter(|c| !std::path::Path::new(c).exists()).collect();
    if !absent.is_empty() {
        return Check::missing(
            "ffmpeg",
            format!(
                "{} dylib(s) que o host pede não existem aqui, a começar por {}",
                absent.len(),
                absent[0]
            ),
        );
    }
    match requested.iter().find(|c| c.contains("libavcodec")) {
        Some(c) => Check::ok("ffmpeg", format!("{} dylib(s) do brew resolvem; {c}", requested.len())),
        // Without libavcodec the binary is not even the screen host -- or
        // it is a build for another platform. Saying "ok" here would pass
        // what was not measured.
        None => Check::missing("ffmpeg", "o logistream-host instalado não pede libavcodec"),
    }
}

fn identity() -> Check {
    let kc = crate::sys::home_dir().join(KEYCHAIN);
    if !kc.exists() {
        return Check::missing("identidade", format!("sem o chaveiro {}", kc.display()));
    }
    // Unlock before asking. Over SSH the keychain is locked and
    // `security` answers "User interaction is not allowed" -- which is
    // not "does not exist". The password is not a secret on purpose (see
    // mac/criar-identidade.sh): the keychain holds ONE self-signed key
    // whose only power is signing our binaries.
    let _ = run_timed("/usr/bin/security", &["unlock-keychain", "-p", "logi", &kc.to_string_lossy()], 8);

    // `find-identity` WITHOUT `-v`.
    //
    // With `-v`, `security` lists only **valid** identities, and valid
    // there means trusted by the system. A self-signed certificate never
    // is: it comes out as `CSSMERR_TP_NOT_TRUSTED` and `-v` returns
    // "0 valid identities found". Measured on the MacBook, with the
    // identity present and the binary signed by it in the same minute.
    //
    // This matters because the wrong diagnosis said to recreate the
    // identity -- and recreating generates a new key, changes the
    // designated requirement, and TCC DISCARDS the Screen Recording
    // grant. The fix would have caused exactly the damage that
    // `criar-identidade.sh` is idempotent to avoid.
    match run_timed("/usr/bin/security", &["find-identity", &kc.to_string_lossy()], 8) {
        Ok(s) if s.contains(SIGNING_IDENTITY) => {
            Check::ok("identidade", format!("\"{SIGNING_IDENTITY}\" no chaveiro próprio"))
        }
        Ok(_) => Check::missing("identidade", format!("o chaveiro existe mas não tem \"{SIGNING_IDENTITY}\"")),
        Err(e) => Check::error("identidade", format!("security: {e}")),
    }
}

fn signature() -> Check {
    let target = host_bin();
    if !target.exists() {
        return Check::missing("assinatura", "o host de tela não está instalado");
    }
    // `codesign -dvvv`, not `-dv`.
    //
    // Two findings on the same line. (1) `codesign` writes everything to
    // STDERR, even on success -- the first version of this read only
    // stdout and thought nothing was signed. (2) `-dv` **does not print
    // the `Authority` line**; it only appears from `-dvv` on. With `-dv`
    // the output has neither "Authority", nor "adhoc", nor "not signed",
    // and the step fell into the "did not recognize" branch -- on a
    // correctly signed binary.
    let output = crate::sys::child("/usr/bin/codesign")
        .args(["-dvvv", &target.to_string_lossy()])
        .output();
    let Ok(o) = output else {
        return Check::error("assinatura", "não consegui executar o codesign");
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    if text.contains(SIGNING_IDENTITY) {
        Check::ok("assinatura", "assinado com a identidade estável")
    } else if text.contains("adhoc") || text.contains("linker-signed") {
        Check::missing(
            "assinatura",
            "ad-hoc / linker-signed: a permissão de tela morre no próximo deploy",
        )
    } else if text.contains("not signed") {
        Check::missing("assinatura", "sem assinatura nenhuma")
    } else {
        Check::error("assinatura", "não reconheci a saída do codesign")
    }
}

/// Reads both permissions from the host itself.
/// Runs the host's `--conferir` as its own launchd job.
///
/// As a child of `logid`, the answer would be about `logid`: macOS
/// attributes the permission to the responsible process. The host's
/// grant was right and the wizard said "missing", because `logid`'s was
/// for an old binary. It is the same question the host's LaunchAgent
/// answers.
fn check_outside_logid(target: &std::path::Path) -> Result<String, String> {
    const LABEL: &str = "ai.logi.conferir";
    let file = crate::sys::config_dir().join("conferir.out");
    let path = file.to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&file);
    let _ = run_timed("/bin/launchctl", &["remove", LABEL], 5);
    run_timed(
        "/bin/launchctl",
        &[
            "submit", "-l", LABEL, "-o", &path, "-e", &path, "--",
            &target.to_string_lossy(), "--conferir",
        ],
        5,
    )?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut text = String::new();
    while std::time::Instant::now() < deadline {
        text = std::fs::read_to_string(&file).unwrap_or_default();
        if text.contains("acessibilidade: ") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = run_timed("/bin/launchctl", &["remove", LABEL], 5);
    let _ = std::fs::remove_file(&file);
    if text.is_empty() {
        return Err("o host não respondeu em 8 s".into());
    }
    Ok(text)
}

fn check_permissions() -> (Check, Check) {
    let target = host_bin();
    if !target.exists() {
        let m = |id: &str| Check::error(id, "o host de tela não está instalado: não há a quem perguntar");
        return (m("gravacao-de-tela"), m("acessibilidade"));
    }
    // 10 s because the binary only prints two lines and exits; if it
    // takes longer, something is hung, and hung is not "ok".
    let output = check_outside_logid(&target);
    let read = |text: &str, key: &str| -> Option<bool> {
        text
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}: ")))
            .map(|v| v.trim() == "ok")
    };
    match output {
        Ok(text) => {
            let one = |id: &str, kind: &str| match read(&text, id) {
                Some(true) => Check::ok(id, format!("concedida a {}", target.display())),
                Some(false) => Check::missing(id, format!("o host não tem {kind}")),
                // An old binary, from before `--conferir`: not measuring
                // is different from measuring "missing", and saying which
                // case it is saves the afternoon I would lose looking in
                // the wrong place.
                None => Check::error(id, "o host instalado não entende --conferir: rode o deploy"),
            };
            (
                one("gravacao-de-tela", "Gravação de Tela"),
                one("acessibilidade", "Acessibilidade"),
            )
        }
        Err(e) => (
            Check::error("gravacao-de-tela", format!("--conferir: {e}")),
            Check::error("acessibilidade", format!("--conferir: {e}")),
        ),
    }
}

fn launchagent() -> Check {
    let label = "ai.logi.logistream-host";
    match run_timed("/bin/launchctl", &["print", &format!("gui/{}/{label}", uid())], 5) {
        Ok(_) => Check::ok("launchagent", format!("{label} carregado")),
        Err(e) => Check::missing("launchagent", format!("{label}: {e}")),
    }
}

fn remote_login() -> Check {
    // A TCP connection to 127.0.0.1:22, not `systemsetup
    // -getremotelogin`: `systemsetup` asks for Full Disk Access on recent
    // macOS and would fail for a reason that has nothing to do with the
    // question. What answers "does this Mac accept SSH?" is the socket.
    let (up, ms) = crate::probe::tcp_probe("127.0.0.1", 22);
    Check::when(
        "login-remoto",
        up,
        format!("sshd responde em {:.0} ms", ms.unwrap_or(0.0)),
        "nada escutando na porta 22",
    )
}

fn tailscale() -> Check {
    // Only the brew paths. The `/usr/local/bin/tailscale` the app
    // installs is a script that opens the app, and opening the app is
    // what triggers the VPN prompt: measuring through it changes what is
    // being measured.
    let app = std::path::Path::new("/Applications/Tailscale.app").exists();
    let cli = ["/opt/homebrew/bin/tailscale", "/usr/local/opt/tailscale/bin/tailscale"]
        .into_iter()
        .find(|c| std::path::Path::new(c).exists());
    let Some(cli) = cli else {
        return Check::missing("tailscale", "tailscale do brew não instalado");
    };
    match run_timed(cli, &["status", "--json"], 5) {
        Ok(json) => crate::setup_common::tailscale_verdict(&json, app),
        Err(e) => Check::missing("tailscale", format!("o tailscaled não respondeu: {e}")),
    }
}

// ------------------------------------------------------------------ repairs

pub fn repair(id: &str, _cfg: &Config) -> Result<String, String> {
    match id {
        "config" => crate::setup_common::write_config(),
        "clipboard" => crate::setup_common::test_clipboard(),
        "daemon" => install_agent(
            "ai.logi.logid",
            &[bin_dir().join("logid").to_string_lossy().into_owned()],
            true,
            None,
        ),
        "launchagent" => install_agent(
            "ai.logi.logistream-host",
            &[
                host_bin().to_string_lossy().into_owned(),
                "9777".into(),
                "Capture screen 0".into(),
            ],
            false,
            None,
        ),
        "ota" => install_clock(),
        "tailscale" => install_tailscale(),
        "assinatura" => sign(),
        "gravacao-de-tela" => request_permission("gravacao-de-tela", "Privacy_ScreenCapture"),
        "acessibilidade" => request_permission("acessibilidade", "Privacy_Accessibility"),
        _ => Err(format!("'{id}' não se arruma sozinho nesta máquina")),
    }
}

/// Writes the plist and loads the agent.
///
/// Generates the XML here instead of copying the file from the repo
/// because the versioned plists have `/Users/user` baked in -- an
/// absolute path cannot be relative, and `~` does not expand in a plist.
/// Generated, the same wizard serves any account.
fn install_agent(
    label: &str,
    argv: &[String],
    run_at_load: bool,
    interval: Option<u64>,
) -> Result<String, String> {
    let home = crate::sys::home_dir();
    let log = home.join(".local/state").join(format!("{}.log", label.rsplit('.').next().unwrap_or(label)));
    std::fs::create_dir_all(home.join(".local/state")).map_err(|e| format!("criar ~/.local/state: {e}"))?;

    let args: String = argv
        .iter()
        .map(|a| format!("    <string>{}</string>\n", escape(a)))
        .collect();
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <!-- Generated by the setup wizard (logid/src/setup_mac.rs). -->\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key><string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n\
         \x20 <key>RunAtLoad</key><{ral}/>\n\
         \x20 <key>KeepAlive</key><{ral}/>\n\
         {interval}\
         \x20 <key>ProcessType</key><string>Interactive</string>\n\
         \x20 <key>StandardOutPath</key><string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key><string>{log}</string>\n\
         </dict>\n</plist>\n",
        ral = if run_at_load { "true" } else { "false" },
        // `StartInterval` and `RunAtLoad false` together: launchd fires
        // the first time only after the interval. That is what we want
        // here -- firing on `bootstrap` would make the agent apply a new
        // build and restart `logid` in the middle of the request that
        // installed it.
        interval = match interval {
            Some(s) => format!("  <key>StartInterval</key><integer>{s}</integer>\n"),
            None => String::new(),
        },
        log = escape(&log.to_string_lossy()),
    );

    let dir = home.join("Library/LaunchAgents");
    std::fs::create_dir_all(&dir).map_err(|e| format!("criar {}: {e}", dir.display()))?;
    let path = dir.join(format!("{label}.plist"));
    std::fs::write(&path, &plist).map_err(|e| format!("gravar {}: {e}", path.display()))?;

    let target = format!("gui/{}", uid());
    let _ = run_timed("/bin/launchctl", &["bootout", &format!("{target}/{label}")], 10);
    // bootstrap fails with an I/O error if the previous one is still
    // exiting. A short pause covers it, and retrying is idempotent.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let p = path.to_string_lossy().into_owned();
    if let Err(e) = run_timed("/bin/launchctl", &["bootstrap", &target, &p], 15) {
        if run_timed("/bin/launchctl", &["bootstrap", &target, &p], 15).is_err() {
            return Err(format!("escrevi o plist, mas o bootstrap falhou: {e}"));
        }
    }
    // Check that it really loaded. A `bootstrap` that returns 0 and an
    // agent that does not show up in `print` is a pair I have seen.
    run_timed("/bin/launchctl", &["print", &format!("{target}/{label}")], 5)
        .map_err(|e| format!("carreguei e o launchd não o lista: {e}"))?;
    Ok(format!("{label} instalado e carregado"))
}

/// Installs the `logi-ota` LaunchAgent and checks once.
///
/// One-hour interval. `RunAtLoad false`: `bootstrap` must not fire the
/// agent now, because it would apply the new build and restart `logid`
/// -- which is what is answering this request. The first check here is
/// `--conferir`, which swaps nothing; the swap happens on the first
/// tick, with nobody waiting.
///
/// Accepted and documented consequence: after a boot, the Mac may take
/// up to an hour to look. For someone travelling with the laptop, that
/// is "when I open the lid and go make coffee".
fn install_clock() -> Result<String, String> {
    let bin = bin_dir().join("logi-ota");
    if !bin.is_file() {
        return Err(format!("não achei {}: rode o deploy", bin.display()));
    }
    install_agent(
        "ai.logi.ota",
        &[bin.to_string_lossy().into_owned()],
        false,
        Some(3600),
    )?;
    clock()?;
    let first = run_timed(&bin.to_string_lossy(), &["--conferir"], 60);
    Ok(match first {
        Ok(s) => format!("agente instalado; primeira checagem: {}", s.trim()),
        Err(e) => format!("agente instalado, mas a primeira checagem falhou: {e}"),
    })
}

/// Opens Terminal with the installer embedded in the binary.
///
/// `sudo` and the login need a person, and Terminal is their place. A
/// new machine has no clone of the repo, so both files come from here.
fn install_tailscale() -> Result<String, String> {
    const SCRIPT: &str = include_str!("../setup/instalar-tailscale-mac.sh");
    let dir = crate::sys::config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("criar {dir:?}: {e}"))?;
    let script = dir.join("instalar-tailscale-mac.sh");
    let opener = dir.join("instalar-tailscale.command");
    std::fs::write(&script, SCRIPT).map_err(|e| format!("gravar {script:?}: {e}"))?;
    let command = format!(
        "#!/bin/bash\n\
         export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH\n\
         if ! command -v brew >/dev/null; then\n\
         \x20 echo 'Homebrew is missing: https://brew.sh'\n\
         else\n\
         \x20 brew list tailscale >/dev/null 2>&1 || brew install tailscale\n\
         \x20 sudo bash '{}'\n\
         fi\n\
         read -r -p 'Press Enter to close '\n",
        script.display()
    );
    std::fs::write(&opener, command).map_err(|e| format!("gravar {opener:?}: {e}"))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("permissão de {opener:?}: {e}"))?;
    run_timed("/usr/bin/open", &[&opener.to_string_lossy()], 10)?;
    let m = tailscale();
    match m.state {
        State::Ok => Ok(m.detail),
        _ => Err(format!("{} -- termine no Terminal que abriu e meça de novo", m.detail)),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn sign() -> Result<String, String> {
    let script = bin_dir().join("assinar.sh");
    let target = host_bin();
    if !script.exists() {
        return Err(format!("não achei {}: rode o deploy", script.display()));
    }
    // The script is deliberately non-fatal (it does not break the deploy
    // when the identity is missing), so its exit code does not say
    // whether it signed. The measurement afterwards decides.
    let _ = run_timed("/bin/bash", &[&script.to_string_lossy(), &target.to_string_lossy()], 30);
    match signature().state {
        State::Ok => Ok("assinado com a identidade estável".into()),
        _ => Err("rodei o assinar.sh e o binário continua sem a identidade; \
                  veja ~/.local/state/assinar.log"
            .into()),
    }
}

/// Triggers the system dialog and opens the right settings pane.
///
/// Both, and in this order: the TCC dialog appears **once in the
/// binary's life**. If the person already answered one day (or if the
/// host was ever launched over SSH and the call went through without a
/// dialog), the only way is the UI -- and finding the item in the list is
/// the step where everyone gives up. Always opening the pane costs one
/// more window and saves the afternoon.
fn request_permission(id: &str, pane: &str) -> Result<String, String> {
    let target = host_bin();
    if !target.exists() {
        return Err("o host de tela não está instalado".into());
    }
    let _ = run_timed(&target.to_string_lossy(), &["--pedir"], 20);
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
    let _ = run_timed("/usr/bin/open", &[&url], 10);

    let (screen, input) = check_permissions();
    let m = if id == "gravacao-de-tela" { screen } else { input };
    match m.state {
        State::Ok => Ok(m.detail),
        // NOT an error: the normal flow is the person ticking the box in
        // the window that just opened, and only then does the answer
        // change. Saying "failed" here would teach distrust of a wizard
        // that is right.

        _ => Err(format!(
            "{} -- marque a caixa na janela que abriu e meça de novo",
            m.detail
        )),
    }
}
