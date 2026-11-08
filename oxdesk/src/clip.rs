//! Clipboard shared between the machines.
//!
//! Spec: ../../specs/004-clipboard.md
//!
//! Replaces KDE Connect, which was the only third-party piece on the path
//! and brought a whole daemon (discovery, pairing, notifications, file
//! transfer) for us to use one function.
//!
//! ## Transport: SSH, not a new port
//!
//! `oxdesk`'s API listens only on `127.0.0.1` by design (specs/008).
//! Opening a LAN port for the clipboard would be new surface for data that
//! is sometimes a password. SSH is already the trusted channel between the
//! machines -- it is how the panel probes, wakes the screen host and runs
//! the agent -- so the push is `ssh <peer> oxdesk clip-set` with the text
//! on stdin.
//!
//! The price is ~300 ms per copy. For a clipboard that is invisible: you
//! copy, switch machines, and only then paste.
//!
//! ## Echo loop
//!
//! Without care, A sends to B, B sees the clipboard change and sends it
//! back to A, forever. We keep the hash of the last text we **saw or
//! wrote**; a change that matches it is not resent.

use std::sync::Mutex;

/// What we ask the clipboard thread to publish.
///
/// Text and file are not the same thing in different sizes: text **is**
/// the content and goes in the POST body; a file is a reference, and the
/// destination's clipboard has to point to files that exist on its disk.
/// See [`crate::clip_files`].
#[derive(Debug, Clone)]
pub enum Request {
    Text(String),
    Files(Vec<String>),
}

/// Last text that went through here, in either direction.
///
/// Kept whole and not just the hash because Linux needs it to answer
/// `SelectionTransfer` -- whoever owns the selection has to be able to
/// deliver it when asked.
static LAST: Mutex<Option<String>> = Mutex::new(None);

/// Cap on what travels. A developer's clipboard sometimes carries a whole
/// file pasted by accident, and sending megabytes over SSH on every copy
/// would turn a convenience into a stall.
pub const LIMIT: usize = 256 * 1024;

/// Is the watcher up? `None` = not known yet.
///
/// It exists because "the thread started" and "the watcher works" are
/// different things, and the difference is invisible: on Linux `spawn`
/// returns the channel right away and the thread can die later, in
/// `EnableClipboard`; on macOS and Windows the sampling loop survives a
/// read error with a silent `continue`. In both cases the clipboard stops
/// crossing over and nothing on the panel changes color.
///
/// The one asking is the setup wizard, which needs a measured answer --
/// not the existence of a channel
/// ([[../../.claude/rules/verify-what-arrived]]). The detail reaches the
/// wizard page, so it stays pt-BR.
static WATCH: Mutex<Option<(bool, String)>> = Mutex::new(None);

pub fn note_watch(ok: bool, detail: impl Into<String>) {
    if let Ok(mut g) = WATCH.lock() {
        *g = Some((ok, detail.into()));
    }
}

pub fn watch_status() -> Option<(bool, String)> {
    WATCH.lock().ok().and_then(|g| g.clone())
}

pub fn last() -> Option<String> {
    LAST.lock().ok().and_then(|g| g.clone())
}

/// Records the text and says whether it is NEW.
///
/// One function for both directions: both "read from the local clipboard"
/// and "received from a peer" go through here, and it is the `false` that
/// cuts the echo loop.
pub fn record(text: &str) -> bool {
    let Ok(mut g) = LAST.lock() else { return false };
    if g.as_deref() == Some(text) {
        return false;
    }
    *g = Some(text.to_string());
    true
}

/// Where `oxdesk` lives on each peer, and how to invoke it.
///
/// Sending just `oxdesk clip-set` does not work: the non-interactive SSH
/// shell does not load the login PATH, and the Mac answered
/// `zsh:1: command not found: oxdesk`. The config's `agent` field does not
/// work either -- it points to `oxdesk-agent.exe` and only exists on
/// Windows (the Mac omits it on purpose).
///
/// So: the path the deploy itself installs, per system.
fn remote_command(h: &crate::config::HostCfg) -> String {
    crate::remote::oxdesk_command(h, "clip-set")
}

/// Pushes the text to the other hosts, over SSH.
///
/// In parallel, one thread per peer: three machines in series would add up
/// three SSH handshakes, and the clipboard would be out of sync for almost
/// a second on the last one.
pub fn push(cfg: &crate::config::Config, text: &str) {
    let targets: Vec<(String, String, String)> = cfg
        .hosts
        .iter()
        .filter(|h| {
            !h.local.unwrap_or(false)
                && !matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1")
        })
        .filter_map(|h| h.ssh.clone().map(|s| (h.name.clone(), s, remote_command(h))))
        .collect();

    for (name, dest, command) in targets {
        let t = text.to_string();
        std::thread::spawn(move || {
            use std::io::Write;
            use std::process::Stdio;
            let child = crate::sys::child("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=6", &dest])
                .arg(&command)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn();
            let Ok(mut child) = child else {
                crate::say!("clip: could not talk to {name}");
                return;
            };
            if let Some(mut e) = child.stdin.take() {
                let _ = e.write_all(t.as_bytes());
            }
            match child.wait_with_output() {
                Ok(o) if o.status.success() => {}
                Ok(o) => crate::say!(
                    "clip: {name} refused ({}): {}",
                    o.status,
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => crate::say!("clip: {name} failed: {e}"),
            }
        });
    }
}

// --- Linux: Mutter RemoteDesktop session -----------------------------
//
// No wl-clipboard and no portal: Wayland does not let you read the
// selection without focus, and the `org.freedesktop.portal.Clipboard`
// portal requires a session with a permission dialog. Mutter's private
// API -- the same one we already use to inject input -- provides
// `EnableClipboard`, `SelectionRead`, `SetSelection` and the signals,
// without asking anything.
//
// Two gotchas, both found by probing:
//
// 1. `EnableClipboard` WITH `mime-types` declares us OWNERS of the
//    selection, and reading starts failing with "Tried to read own
//    selection". To observe, call it without types; declare types only
//    when writing.
// 2. The `SelectionRead` fd comes **non-blocking**. `read_to_end` on it
//    returns `WouldBlock` right away -- and whoever swallows that error
//    reads zero bytes and concludes the clipboard is empty. That is what
//    happened on the first probe.
#[cfg(target_os = "linux")]
pub mod linux {
    use dbus::arg::{RefArg, Variant};
    use dbus::blocking::Connection;
    use dbus::Path;
    use std::collections::HashMap;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::sync::Arc;
    use std::time::Duration;

    const RD: &str = "org.gnome.Mutter.RemoteDesktop";
    const RD_OBJ: &str = "/org/gnome/Mutter/RemoteDesktop";
    const RDS: &str = "org.gnome.Mutter.RemoteDesktop.Session";
    const MIME: &str = "text/plain;charset=utf-8";
    /// Files: the URI list, which is what Wayland and GTK speak.
    const URI: &str = "text/uri-list";
    /// What Nautilus reads when pasting. First line `copy` or `cut`.
    const GNOME: &str = "x-special/gnome-copied-files";
    const T: Duration = Duration::from_secs(5);

    /// Reads an fd Mutter handed over, clearing O_NONBLOCK first.
    fn read_fd(fd: RawFd) -> std::io::Result<Vec<u8>> {
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 {
            return Err(std::io::Error::last_os_error());
        }
        unsafe {
            let fl = libc::fcntl(dup, libc::F_GETFL);
            libc::fcntl(dup, libc::F_SETFL, fl & !libc::O_NONBLOCK);
        }
        let mut f = unsafe { std::fs::File::from_raw_fd(dup) };
        let mut v = Vec::new();
        f.read_to_end(&mut v)?;
        Ok(v)
    }

    fn mime_types(which: &[&str]) -> HashMap<&'static str, Variant<Box<dyn RefArg>>> {
        let mut m: HashMap<&str, Variant<Box<dyn RefArg>>> = HashMap::new();
        let v: Vec<String> = which.iter().map(|s| s.to_string()).collect();
        m.insert("mime-types", Variant(Box::new(v)));
        m
    }

    /// What we are publishing now, to answer the transfer.
    ///
    /// Whoever owns the selection has to be able to deliver it when asked,
    /// and the request comes by MIME: Nautilus asks for
    /// `x-special/gnome-copied-files`, a terminal asks for `text/uri-list`,
    /// an editor asks for `text/plain`. Without keeping what we published,
    /// the paste arrives and there is nothing to answer with.
    static PUBLISHED: std::sync::Mutex<Option<super::Request>> = std::sync::Mutex::new(None);

    /// The bytes that answer the requested MIME, if we know how to answer it.
    fn serve(mime: &str) -> Option<Vec<u8>> {
        let g = PUBLISHED.lock().ok()?;
        match g.as_ref()? {
            super::Request::Text(t) => {
                if mime.starts_with("text/plain") || mime == "UTF8_STRING" {
                    Some(t.as_bytes().to_vec())
                } else {
                    None
                }
            }
            super::Request::Files(v) => {
                if mime == GNOME {
                    Some(crate::clip_files::gnome_copied_files(v).into_bytes())
                } else if mime == URI {
                    Some(crate::clip_files::uris_from_paths(v).into_bytes())
                } else if mime.starts_with("text/plain") {
                    // Pasting a file into an editor gives the paths. It is
                    // what GTK does, and what the person expects.
                    Some(v.join("\n").into_bytes())
                } else {
                    None
                }
            }
        }
    }

    /// Reads the current selection in a MIME, or `None`.
    ///
    /// The fd comes **non-blocking** from Mutter, and `read_to_end` on it
    /// returns `WouldBlock` right away -- whoever swallows that error reads
    /// zero bytes and concludes the clipboard is empty. That is what
    /// happened on the first probe; `read_fd` clears O_NONBLOCK.
    fn read_selection(c: &Connection, sess: &Path<'static>, mime: &str) -> Option<Vec<u8>> {
        let r: Result<(dbus::arg::OwnedFd,), _> = c
            .with_proxy(RD, sess.clone(), T)
            .method_call(RDS, "SelectionRead", (mime,));
        let (fd,) = r.ok()?;
        read_fd(fd.as_raw_fd()).ok()
    }

    /// Walks down a `RefArg` collecting every string.
    ///
    /// `as_iter` goes down **one** level and the list comes as `(as)`
    /// inside a variant, two levels below. The first version of this
    /// printed an empty list and made me doubt Mutter instead of my code.
    fn collect_strings(a: &dyn RefArg, out: &mut Vec<String>) {
        if let Some(s) = a.as_str() {
            out.push(s.to_string());
            return;
        }
        if let Some(it) = a.as_iter() {
            for x in it {
                collect_strings(&x, out);
            }
        }
    }

    /// Clipboard thread: observes, publishes and answers transfers.
    ///
    /// All in one thread because the D-Bus connection **is not Send** and
    /// the session belongs to the connection that created it -- the same
    /// restriction that shaped the screen host. Whoever wants to write
    /// sends through the channel.
    pub fn spawn(cfg: Arc<std::sync::RwLock<crate::config::Config>>) -> std::sync::mpsc::Sender<super::Request> {
        let (tx, rx) = std::sync::mpsc::channel::<super::Request>();
        std::thread::spawn(move || {
            match run(&cfg, &rx) {
                Err(e) => {
                    crate::say!("clip: shut down ({e})");
                    super::note_watch(false, e.to_string());
                }
                // Exiting without error is still exiting: the channel
                // closed and from here on nothing crosses over.
                Ok(()) => super::note_watch(false, "a thread do clipboard encerrou"),
            }
        });
        tx
    }

    fn run(
        cfg: &Arc<std::sync::RwLock<crate::config::Config>>,
        rx: &std::sync::mpsc::Receiver<super::Request>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let c = Connection::new_session()?;
        let (sess,): (Path,) =
            c.with_proxy(RD, RD_OBJ, T).method_call(RD, "CreateSession", ())?;
        c.with_proxy(RD, sess.clone(), T).method_call::<(), _, _, _>(RDS, "Start", ())?;
        // No types: we want to OBSERVE. Declaring types here would make us
        // owners of the selection and blind to everyone else's.
        let empty: HashMap<&str, Variant<Box<dyn RefArg>>> = HashMap::new();
        c.with_proxy(RD, sess.clone(), T)
            .method_call::<(), _, _, _>(RDS, "EnableClipboard", (empty,))?;
        crate::say!("clip: watching the clipboard (session {sess})");
        super::note_watch(true, format!("sessão {sess} do Mutter aceita"));

        // Someone copied something on this machine.
        let owner = {
            let s = sess.clone();
            dbus::message::MatchRule::new_signal(RDS, "SelectionOwnerChanged")
                .with_path(s)
        };
        c.add_match(owner, |_: (), _, _| true)?;
        // Someone asked for what WE published.
        let transfer = dbus::message::MatchRule::new_signal(RDS, "SelectionTransfer")
            .with_path(sess.clone());
        c.add_match(transfer, |_: (), _, _| true)?;

        loop {
            // A write requested by a peer, if any.
            while let Ok(request) = rx.try_recv() {
                let which: &[&str] = match &request {
                    super::Request::Text(t) => {
                        if !super::record(t) {
                            continue;
                        }
                        &[MIME]
                    }
                    // Both types together: Nautilus only pastes through
                    // `gnome-copied-files`, and non-GTK programs only
                    // understand `uri-list`. Publishing just one leaves half
                    // the programs unable to paste.
                    super::Request::Files(_) => &[GNOME, URI, MIME],
                };
                if let Ok(mut g) = PUBLISHED.lock() {
                    *g = Some(request);
                }
                let _ = c
                    .with_proxy(RD, sess.clone(), T)
                    .method_call::<(), _, _, _>(RDS, "SetSelection", (mime_types(which),));
            }

            // One D-Bus turn. 300 ms is a compromise: less than that spins
            // the CPU for nothing, more than that delays answering a
            // transfer -- and whoever asked for the selection is waiting.
            let msg = c.channel().blocking_pop_message(Duration::from_millis(300))?;
            let Some(m) = msg else { continue };
            let member = m.member().map(|s| s.to_string()).unwrap_or_default();

            if member == "SelectionOwnerChanged" {
                // The signal says WHICH types the owner published, and
                // whether the owner is us. Measured with `oxdesk clip-sonda`:
                //
                //   text: ["text/plain;charset=utf-8", "UTF8_STRING", "TARGETS"]
                //   file: ["text/uri-list", "TARGETS"]
                //
                // Using that list is the only honest way to know what was
                // copied: `SelectionRead("text/uri-list")` returns **OK even
                // on a text copy**, so "the read worked" tells nothing apart.
                let opts: HashMap<String, Variant<Box<dyn RefArg>>> =
                    m.read1().unwrap_or_default();
                if opts
                    .get("session-is-owner")
                    .and_then(|v| v.0.as_u64())
                    .is_some_and(|x| x == 1)
                {
                    continue; // it is ours; we published it ourselves
                }
                let mut offered: Vec<String> = Vec::new();
                if let Some(v) = opts.get("mime-types") {
                    collect_strings(&*v.0, &mut offered);
                }
                let has = |t: &str| offered.iter().any(|x| x == t);

                // Read right away: the selection belongs to the CURRENT
                // owner, and if it leaves before we read, the content is gone.
                if has(URI) {
                    let Some(bytes) = read_selection(&c, &sess, URI) else { continue };
                    let Ok(list) = String::from_utf8(bytes) else { continue };
                    let paths = crate::clip_files::paths_from_uris(&list);
                    if paths.is_empty() {
                        continue;
                    }
                    if let Ok(g) = cfg.read() {
                        crate::clip_files::on_copy(&g, &paths);
                    }
                    continue;
                }
                if !offered.is_empty() && !offered.iter().any(|t| t.starts_with("text/plain"))
                {
                    continue; // image, HTML, something we do not handle
                }
                let Some(bytes) = read_selection(&c, &sess, MIME) else { continue };
                if bytes.is_empty() || bytes.len() > super::LIMIT {
                    continue;
                }
                let Ok(text) = String::from_utf8(bytes) else { continue };
                if super::record(&text) {
                    if let Ok(g) = cfg.read() {
                        super::push(&g, &text);
                    }
                }
            } else if member == "SelectionTransfer" {
                // (mime, serial) -- someone wants to paste what we published.
                let (mime, serial): (String, u32) = match m.read2() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                // Answer by the REQUESTED MIME, not always the same one:
                // Nautilus asks for `gnome-copied-files` and an editor asks
                // for `text/plain`, from the same selection.
                let content = serve(&mime)
                    .unwrap_or_else(|| super::last().unwrap_or_default().into_bytes());
                let r: Result<(dbus::arg::OwnedFd,), _> = c
                    .with_proxy(RD, sess.clone(), T)
                    .method_call(RDS, "SelectionWrite", (serial,));
                let ok = match r {
                    Ok((fd,)) => {
                        use std::io::Write;
                        let dup = unsafe { libc::dup(fd.as_raw_fd()) };
                        let mut f = unsafe { std::fs::File::from_raw_fd(dup) };
                        f.write_all(&content).is_ok()
                    }
                    Err(_) => false,
                };
                let _ = c
                    .with_proxy(RD, sess.clone(), T)
                    .method_call::<(), _, _, _>(RDS, "SelectionWriteDone", (serial, ok));
            }
        }
    }
}

// --- local write, per system -----------------------------------------

/// Writes to THIS machine's clipboard.
///
/// On Linux the D-Bus thread is the one that writes (the session belongs
/// to the connection that created it), so here we only push through the
/// channel. On the other two we use the tools the system itself ships:
/// `clip.exe` and `pbcopy` exist on every install, and writing the
/// clipboard through a native API would be our code doing what the system
/// already does the same way.
#[cfg(target_os = "linux")]
pub fn set_local(channel: &std::sync::mpsc::Sender<Request>, text: &str) -> Result<(), String> {
    channel
        .send(Request::Text(text.to_string()))
        .map_err(|_| "the clipboard thread exited".to_string())
}

#[cfg(not(target_os = "linux"))]
pub fn set_local(_channel: &(), text: &str) -> Result<(), String> {
    #[cfg(not(windows))]
    use std::io::Write;
    #[cfg(not(windows))]
    use std::process::Stdio;
    if !record(text) {
        return Ok(()); // it already is that; writing again would only echo
    }
    #[cfg(windows)]
    {
        return winput::clipboard_write(text);
    }
    #[cfg(not(windows))]
    {
        let mut f = crate::sys::child("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("pbcopy: {e}"))?;
        if let Some(mut e) = f.stdin.take() {
            e.write_all(text.as_bytes()).map_err(|e| format!("pbcopy: {e}"))?;
        }
        f.wait().map_err(|e| format!("pbcopy: {e}"))?;
        Ok(())
    }
}

/// Watches the clipboard by sampling. Windows and macOS.
///
/// By sampling and not by event because both have notification APIs
/// (`AddClipboardFormatListener`, `NSPasteboard` `changeCount`) that would
/// require a message loop or objc inside a daemon that has neither. Two
/// seconds is the compromise: you cannot feel it when switching machines,
/// and it is 30 reads a minute of a cheap command.
///
/// Linux does not go through here -- there the `SelectionOwnerChanged`
/// signal tells right away.
#[cfg(not(target_os = "linux"))]
pub fn watch(cfg: std::sync::Arc<std::sync::RwLock<crate::config::Config>>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        // `crate::sys::child`, NEVER `Command::new` directly.
        //
        // On Windows that helper sets `CREATE_NO_WINDOW`; without it each
        // sample opens a visible console window. With a 2 s interval that
        // became "o windows ta abrindo um powershell a cada 5s" ("Windows
        // is opening a PowerShell every 5s") on the user's screen -- an
        // invisible process blinking 30 times a minute.
        // File BEFORE text, not after.
        //
        // Both live on the clipboard together: Explorer publishes `CF_HDROP`
        // along with the file name as text, and Finder does the equivalent.
        // Looking at text first, copying a file would send the PATH as a
        // string to the peers -- a path that does not exist there -- and
        // the file would never go.
        //
        // On Windows these are two Win32 calls, no process at all. On macOS
        // it is ONE call that returns both, because there the cost is the
        // process: `osascript` costs 55 ms against 11 ms for `pbpaste`
        // (measured), and running both every 2 s would be 3.3% of a core
        // forever on a laptop.
        let text = {
            #[cfg(windows)]
            {
                let files = crate::clip_files::read_local();
                if !files.is_empty() {
                    if let Ok(g) = cfg.read() {
                        crate::clip_files::on_copy(&g, &files);
                    }
                    continue;
                }
                match winput::clipboard_read() {
                    Ok(Some(t)) => {
                        note_watch(true, "a area de transferencia responde");
                        t
                    }
                    // Empty is healthy: opening worked, there was nothing.
                    Ok(None) => {
                        note_watch(true, "a area de transferencia responde (vazia)");
                        continue;
                    }
                    // A silent `continue` used to be here. A read error on
                    // Windows is almost always a session boundary -- the
                    // daemon in session 0 sees an always-empty clipboard --
                    // and that has to show on the panel, not vanish.
                    Err(e) => {
                        note_watch(false, e.to_string());
                        continue;
                    }
                }
            }
            #[cfg(not(windows))]
            {
                match crate::clip_files::read_clipboard() {
                    crate::clip_files::Clipboard::Files(v) => {
                        if let Ok(g) = cfg.read() {
                            crate::clip_files::on_copy(&g, &v);
                        }
                        continue;
                    }
                    crate::clip_files::Clipboard::Text(t) => t,
                    crate::clip_files::Clipboard::Empty => continue,
                }
            }
        };
        let text = text.trim_end_matches(['\r', '\n']).to_string();
        if text.is_empty() || text.len() > LIMIT {
            continue;
        }
        if record(&text) {
            if let Ok(g) = cfg.read() {
                push(&g, &text);
            }
        }
    });
}

/// `oxdesk clip-set`: reads the text from stdin and hands it to the local daemon.
///
/// Goes through the API on 127.0.0.1 instead of writing the clipboard
/// directly, and that is necessary on Linux: whoever writes has to STAY
/// ALIVE to answer `SelectionTransfer` when someone pastes. A CLI process
/// that writes and exits would leave a selection with no owner -- the text
/// would vanish on the first Ctrl+V. So the CLI only hands it to the
/// daemon, which stays.
///
/// Raw HTTP over a TcpStream instead of an HTTP client: twelve lines
/// against a whole dependency, for one POST to localhost.
pub fn cli_set(port: u16) -> std::process::ExitCode {
    use std::io::{Read, Write};
    let mut text = String::new();
    if std::io::stdin().read_to_string(&mut text).is_err() {
        eprintln!("clip-set: could not read stdin");
        return std::process::ExitCode::FAILURE;
    }
    if text.len() > LIMIT {
        eprintln!("clip-set: {} bytes over the {LIMIT} limit", text.len());
        return std::process::ExitCode::FAILURE;
    }
    let body = text.into_bytes();
    let request = format!(
        "POST /api/clipboard HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut s = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("clip-set: the local oxdesk did not answer on port {port}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if s.write_all(request.as_bytes()).is_err() || s.write_all(&body).is_err() {
        eprintln!("clip-set: failed to write the request");
        return std::process::ExitCode::FAILURE;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    if resp.starts_with("HTTP/1.1 200") {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("clip-set: {}", resp.lines().next().unwrap_or("no response"));
        std::process::ExitCode::FAILURE
    }
}

// --- bridge between the HTTP handler and the thread ------------------
//
// The handler runs on one of the server's four threads and needs to reach
// the clipboard thread. Keeping the channel in a static is simpler than
// carrying one more `Arc` through the whole `handle` signature, which
// today takes only config and cache.

#[cfg(target_os = "linux")]
static CHANNEL: Mutex<Option<std::sync::mpsc::Sender<Request>>> = Mutex::new(None);

#[cfg(target_os = "linux")]
pub fn store_channel(c: std::sync::mpsc::Sender<Request>) {
    if let Ok(mut g) = CHANNEL.lock() {
        *g = Some(c);
    }
}

/// Writes to the local clipboard what came from a peer.
pub fn apply_local(text: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let g = CHANNEL.lock().map_err(|_| "channel poisoned".to_string())?;
        let c = g.as_ref().ok_or("the clipboard thread did not start")?;
        set_local(c, text)
    }
    #[cfg(not(target_os = "linux"))]
    {
        set_local(&(), text)
    }
}

/// Publishes files on the local clipboard (Linux).
///
/// Goes through the D-Bus thread because the session belongs to the
/// connection that created it, and because whoever publishes has to stay
/// alive to answer `SelectionTransfer` at paste time.
#[cfg(target_os = "linux")]
pub fn publish_files(paths: &[String]) -> Result<(), String> {
    let g = CHANNEL.lock().map_err(|_| "channel poisoned".to_string())?;
    let c = g.as_ref().ok_or("the clipboard thread did not start")?;
    c.send(Request::Files(paths.to_vec()))
        .map_err(|_| "the clipboard thread exited".to_string())
}
