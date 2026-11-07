//! Setup measurement and repair, on Ubuntu.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! The catalog and the decision live in `setup.rs`; here we only measure
//! the world and touch it. Two things the split imposes, and that hold:
//!
//! 1. Every measurement is **cheap and time-limited**. The wizard calls
//!    `measure()` every few seconds; a step that takes 10 s brings the
//!    whole panel down. Where the honest measurement is expensive (NVENC,
//!    SSH), it comes from a cache or from a probe with a timeout.
//! 2. Every repair returns the message of what it **measured
//!    afterwards**, not of what it ordered. "I ran the command" is not a
//!    result ([[../../.claude/rules/verify-what-arrived]]).

use crate::config::Config;
use crate::setup::{State, Check};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const BINS: [&str; 6] =
    ["logid", "logistream-host", "logistream-view", "logistream-ponte", "logistream-kvm", "logi-ota"];

fn bin_dir() -> PathBuf {
    crate::sys::home_dir().join(".local/bin")
}

pub fn measure(cfg: &Config) -> Vec<Check> {
    vec![
        binaries(),
        crate::setup_common::this_machine_config(cfg),
        daemon(),
        receiver(),
        input_kvm(),
        wayland(),
        crate::setup_common::clipboard(),
        crate::setup_common::peers(cfg),
        nvenc(),
        tray(),
        ota(),
    ]
}

fn ota_token() -> PathBuf {
    crate::sys::home_dir().join(".config/logi/ota-token")
}

fn ota() -> Check {
    crate::setup_common::ota(&bin_dir().join("logi-ota"), &ota_token(), clock())
}

/// The systemd timer that wakes `logi-ota` up.
///
/// `is-active` **and** `is-enabled`: a timer that is active but not
/// enabled dies on the next boot without warning -- the same pair the
/// daemon step checks, for the same reason.
fn clock() -> Result<String, String> {
    let active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "logi-ota.timer"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !active {
        return Err("logi-ota.timer não está ativo".into());
    }
    let enabled = Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", "logi-ota.timer"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !enabled {
        return Err("logi-ota.timer roda agora mas não está habilitado: no próximo boot não sobe".into());
    }
    Ok("timer do systemd a cada 1 h".into())
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
    let state = crate::sys::unit_state("logid.service");
    let enabled = Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", "logid.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    match (state.as_str(), enabled) {
        ("active", true) => Check::ok("daemon", "ativo e habilitado no logon"),
        // Active but not enabled is the treacherous case: it works today
        // and disappears on the next boot. It must weigh as missing.
        ("active", false) => Check::missing("daemon", "ativo, mas NÃO habilitado: não volta no próximo logon"),
        (other, _) => Check::missing("daemon", format!("unit {other}")),
    }
}

/// Does any udev rule cover `hidraw` for Logitech devices?
///
/// Scans at FILE level, not line level, and all three directories. The
/// first version of this looked only at `/etc/udev/rules.d` and required
/// `hidraw` and `046d` on the SAME line -- and said "no rule" on a
/// machine where access worked. The real rule (the one from the Solaar
/// package, in `/usr/lib`) uses `GOTO`, so the subsystem and the vendor
/// appear on different lines. Wrong measurement, right machine.
fn hidraw_rule() -> Option<String> {
    for dir in ["/etc/udev/rules.d", "/usr/lib/udev/rules.d", "/lib/udev/rules.d"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let useful: String = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with('#') && !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if useful.contains("hidraw")
                && useful.contains("046d")
                && (useful.contains("uaccess") || useful.contains("MODE"))
            {
                return Some(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// (Logitech `hidraw` nodes, how many open).
///
/// Filters by vendor instead of using `sys::hidraw_count`, which counts
/// ALL nodes: on this machine there are four, and one of them is a device
/// from another vendor that does not open -- so the overall count said
/// "3 of 4" and left the impression that a permission was missing
/// somewhere. The vendor comes from `HID_ID` in the uevent
/// (`0003:0000046D:...`), which exists for both USB and Bluetooth.
fn hidraw_logitech() -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir("/dev") else { return (0, 0) };
    let mut total = 0;
    let mut openable = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if !n.starts_with("hidraw") {
            continue;
        }
        let uevent = std::fs::read_to_string(format!("/sys/class/hidraw/{n}/device/uevent"))
            .unwrap_or_default()
            .to_ascii_uppercase();
        let logitech = uevent
            .lines()
            .filter_map(|l| l.strip_prefix("HID_ID="))
            .any(|v| v.split(':').nth(1).is_some_and(|ven| ven.ends_with("046D")));
        if !logitech {
            continue;
        }
        total += 1;
        if std::fs::File::open(e.path()).is_ok() {
            openable += 1;
        }
    }
    (total, openable)
}

/// Is the HID++ receiver accessible NOW, and will it stay so after a
/// replug?
///
/// Two questions in one line on purpose. The ACL alone would answer
/// "works today" and hide that it dies on the next replug; the rule alone
/// would answer "should work" without looking at whether it does.
fn receiver() -> Check {
    let (total, openable) = hidraw_logitech();
    if total == 0 {
        // Zero nodes is absent hardware, not a missing permission. The
        // panel once told the person to "run udevadm" when the receiver
        // was on the other machine -- fixing a permission that was not
        // broken.
        return Check::not_applicable("receptor", "nenhum hidraw Logitech agora: receptor desplugado?");
    }
    let rule = hidraw_rule();
    match (openable > 0, rule) {
        (true, Some(r)) => Check::ok("receptor", format!("{openable} de {total} nós abrem; regra {r}")),
        (true, None) => Check::missing(
            "receptor",
            format!("{openable} de {total} nós abrem AGORA, mas nenhuma regra cobre hidraw+046d: \
                     a ACL some no próximo replug"),
        ),
        (false, Some(r)) => Check::missing(
            "receptor",
            format!("{r} existe e nenhum dos {total} nós abre: falta um udevadm trigger"),
        ),
        (false, None) => Check::missing("receptor", format!("nenhum dos {total} nós de /dev/hidraw abre")),
    }
}

/// Are the Logitech devices' `event*` nodes readable?
///
/// By vendor in `/sys`, not by name: "MX Master" becomes another name
/// when the device switches channel, and a name filter would measure
/// right today and wrong tomorrow.
fn input_kvm() -> Check {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return Check::error("input-kvm", "não consegui ler /dev/input");
    };
    let mut total = 0;
    let mut readable = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if !n.starts_with("event") {
            continue;
        }
        let vendor = std::fs::read_to_string(format!("/sys/class/input/{n}/device/id/vendor"))
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if vendor != "046d" {
            continue;
        }
        total += 1;
        if std::fs::File::open(e.path()).is_ok() {
            readable += 1;
        }
    }
    if total == 0 {
        return Check::not_applicable("input-kvm", "nenhum dispositivo Logitech em /dev/input agora");
    }
    Check::when(
        "input-kvm",
        readable == total,
        format!("{readable} de {total} dispositivos Logitech legíveis"),
        format!("só {readable} de {total} dispositivos Logitech são legíveis"),
    )
}

fn wayland() -> Check {
    match std::env::var("XDG_SESSION_TYPE").unwrap_or_default().as_str() {
        "wayland" => Check::ok("wayland", "Wayland"),
        "" => Check::error("wayland", "XDG_SESSION_TYPE vazio: o daemon não herdou o ambiente da sessão"),
        other => Check::missing("wayland", format!("sessão {other}: borda e clipboard não foram validados aqui")),
    }
}

fn nvenc() -> Check {
    // The real probe (`logistream-probe-nvenc`) initializes CUDA and
    // takes seconds. For the panel the earlier question is enough: is an
    // NVIDIA driver loaded? Without it the probe fails anyway, and with
    // it the screen path has already been exercised for real by use.
    if !std::path::Path::new("/dev/nvidiactl").exists() {
        return Check::not_applicable("nvenc", "sem GPU NVIDIA: esta máquina pode assistir, não ser assistida");
    }
    match crate::setup_common::run_timed("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"], 5) {
        Ok(s) => {
            let name = s.lines().next().unwrap_or("").trim().to_string();
            Check::ok("nvenc", if name.is_empty() { "driver carregado".into() } else { name })
        }
        Err(e) => Check::error("nvenc", format!("nvidia-smi: {e}")),
    }
}

fn tray() -> Check {
    // `pgrep -f` with the pattern the deploy skill uses to kill the
    // process. Matching on the name "electron" alone would catch any
    // Electron app on the machine.
    let alive = Command::new("pgrep")
        .args(["-f", "electron \\.$"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    Check::when("bandeja", alive, "rodando", "não está rodando")
}

// ------------------------------------------------------------------ repairs

pub fn repair(id: &str, cfg: &Config) -> Result<String, String> {
    match id {
        "config" => crate::setup_common::write_config(),
        "daemon" => {
            // `enable --now` on a unit that already runs is idempotent,
            // and it is what fixes the "active but not enabled" case
            // without killing the daemon answering this request.
            crate::setup_common::run_timed(
                "systemctl",
                &["--user", "enable", "--now", "logid.service"],
                15,
            )?;
            Ok("unit habilitada".into())
        }
        "clipboard" => crate::setup_common::test_clipboard(),
        "bandeja" => launch_tray(),
        "ota" => install_clock(),
        _ => Err(format!("'{id}' não se arruma sozinho nesta máquina")),
    }
    .map(|s| {
        let _ = cfg;
        s
    })
}

/// Writes the `logi-ota` service and timer and enables them.
///
/// The unit is **generated here**, not copied from the repo: the wizard
/// has to work on a machine without the checkout (the Mac's case). And
/// `%h` resolves to the home of whoever runs it, so the same text serves
/// any account.
///
/// The first run is `--conferir`, on purpose: `logi-ota` without
/// arguments may apply a new build and **restart logid** -- which is
/// what is answering this request. The clock applies on the next tick,
/// with nobody waiting for an answer.
fn install_clock() -> Result<String, String> {
    let bin = bin_dir().join("logi-ota");
    if !bin.is_file() {
        return Err(format!("não achei {}: rode o deploy", bin.display()));
    }
    let dir = crate::sys::home_dir().join(".config/systemd/user");
    std::fs::create_dir_all(&dir).map_err(|e| format!("criar {}: {e}", dir.display()))?;

    // `concat!` and not line continuation with `\`: the previous version
    // wrote the file with the source code's indentation inside. systemd
    // swallowed it and it worked, but a unit with leading whitespace is a
    // time bomb.
    let service = concat!(
        "[Unit]\n",
        "Description=logi-ota - checks the published Release and applies it if newer\n",
        "Documentation=file:///home/user/dev/logi/specs/015-ota.md\n",
        "\n",
        "[Service]\n",
        "Type=oneshot\n",
        "ExecStart=%h/.local/bin/logi-ota\n",
    );
    // `OnActiveSec` and not `OnBootSec`: on a machine that has been on
    // for hours, `OnBootSec=3min` has already passed, and `enable --now`
    // fires the service RIGHT AWAY -- that is, the wizard button would
    // apply a new build and restart `logid` in the middle of the request
    // that installed it. That is what happened here in the first version
    // (`list-timers` showed "LAST 495ms ago" right after the POST).
    // Relative to the timer's activation, the first check lands 10 min
    // later -- after installing or booting the machine -- and nobody is
    // waiting for an answer anymore.
    let timer = concat!(
        "[Unit]\n",
        "Description=logi-ota timer\n",
        "\n",
        "[Timer]\n",
        "OnActiveSec=10min\n",
        "OnUnitActiveSec=1h\n",
        "Unit=logi-ota.service\n",
        "\n",
        "[Install]\n",
        "WantedBy=timers.target\n",
    );

    std::fs::write(dir.join("logi-ota.service"), service)
        .map_err(|e| format!("gravar logi-ota.service: {e}"))?;
    std::fs::write(dir.join("logi-ota.timer"), timer)
        .map_err(|e| format!("gravar logi-ota.timer: {e}"))?;

    crate::setup_common::run_timed("systemctl", &["--user", "daemon-reload"], 15)?;
    crate::setup_common::run_timed("systemctl", &["--user", "enable", "--now", "logi-ota.timer"], 15)?;

    // Check what stuck, not what I ordered.
    clock()?;
    let first = crate::setup_common::run_timed(&bin.to_string_lossy(), &["--conferir"], 60);
    Ok(match first {
        Ok(s) => format!("timer habilitado; primeira checagem: {}", s.trim()),
        Err(e) => format!("timer habilitado, mas a primeira checagem falhou: {e}"),
    })
}

/// Launches the Electron shell.
///
/// `env -u ELECTRON_RUN_AS_NODE` is not optional and not superstition:
/// with that variable in the environment electron runs as plain node,
/// `app` comes back `undefined` and the process dies in
/// `requestSingleInstanceLock()` -- with a message that says nothing
/// about the variable. I already brought the user's GUI down that way,
/// and the daemon inherits the environment of whoever launched it, so
/// the variable may well be present here.
fn launch_tray() -> Result<String, String> {
    let dir = crate::sys::home_dir().join("dev/logi/gui");
    if !dir.join("main.js").exists() {
        return Err(format!("não achei a casca em {}", dir.display()));
    }
    let log = crate::sys::state_dir().join("logi-gui.log");
    let output = std::fs::File::create(&log).map_err(|e| format!("abrir {}: {e}", log.display()))?;
    let error = output.try_clone().map_err(|e| format!("duplicar o log: {e}"))?;
    Command::new("setsid")
        .args(["env", "-u", "ELECTRON_RUN_AS_NODE", "node", "./node_modules/.bin/electron", "."])
        .current_dir(&dir)
        .stdout(output)
        .stderr(error)
        .spawn()
        .map_err(|e| format!("lançar: {e}"))?;

    // Measure after launching, do not announce the launch: electron dies
    // within ~200 ms when it dies, and an immediate "ready" would hide
    // precisely the most common failure.

    std::thread::sleep(Duration::from_millis(1500));
    if tray().state == State::Ok {
        Ok("bandeja no ar".into())
    } else {
        Err(format!("lancei e não ficou de pé; veja {}", log.display()))
    }
}
