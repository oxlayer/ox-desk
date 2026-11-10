//! FILE clipboard between the machines.
//!
//! Spec: ../../specs/004-clipboard.md
//!
//! Sibling of [`crate::clip`], which handles text. Separate because the
//! difference is not one of size, it is one of nature: text **is** the
//! content and travels whole in a POST body; a file is a *reference*, and
//! the destination's clipboard has to point to files that exist on ITS
//! disk. You cannot "paste" a path that only exists on the source machine.
//!
//! Hence the consequence that shapes everything here: the bytes have to
//! arrive before the paste, not during it. There is no way to intercept
//! Ctrl+V on all three systems -- Windows would require delayed rendering
//! of `CF_HDROP`, macOS a live pasteboard provider -- so we copy at Ctrl+C
//! time and leave the files waiting in a staging folder.
//!
//! ## Why a cap, and what it decides
//!
//! Copying a folder in a file manager is a keyboard shortcut. Without a
//! cap, selecting `~/Downloads` by accident would send gigabytes to two
//! machines you are not even looking at -- and the cost is not just
//! bandwidth, it is someone else's disk.
//!
//! There are three limits, and each answers a different question:
//!
//! | Limit | Question |
//! |---|---|
//! | [`INLINE_CAP`] | does it go along with the notice, or does it need its own transfer? |
//! | [`TOTAL_CAP`] | is this a copy or an accident? |
//! | [`FILE_COUNT_CAP`] | same, for a large and light tree (`node_modules`) |
//!
//! ## Transport
//!
//! `tar` over SSH's stdin, on all three machines -- Windows has `tar.exe`
//! in `system32` since 1809. A single trip, no temporary file in between,
//! and it preserves the directory tree.
//!
//! Above [`INLINE_CAP`] we prefer `rsync` **when it exists on both ends**,
//! for `--partial`: on a 2 GB file a Wi-Fi drop in the middle costs the
//! whole transfer with `tar`, and only the rest of it with `rsync`.
//! Windows has no rsync (measured: only `tar.exe` and `scp.exe`), so there
//! it falls back to `tar` and the log says there will be no resume.

use std::path::{Path, PathBuf};

/// Goes inline in the same SSH that notifies the peer.
pub const INLINE_CAP: u64 = 8 * 1024 * 1024;

/// Above this we do not copy: warn and stop.
///
/// 2 GiB is generous on purpose -- moving a video between machines is
/// legitimate use. What it blocks is the wrong order of magnitude: the
/// whole home folder, a photo library, a VM directory.
pub const TOTAL_CAP: u64 = 2 * 1024 * 1024 * 1024;

/// Cap on count, regardless of size.
///
/// It exists because `node_modules` has 40 thousand files and 300 MB: it
/// passes the byte cap easily and still stalls the copy for minutes, a
/// `tar` of 40 thousand headers.
pub const FILE_COUNT_CAP: usize = 2000;

/// How the bytes will travel -- or why they will not.
#[derive(Debug, PartialEq, Eq)]
pub enum Transport {
    /// `tar` on SSH's stdin, together with the notice.
    Inline,
    /// Its own transfer before the notice (`rsync`, or `tar` where there
    /// is none).
    Separate,
    /// We will not copy. The text is the reason, ready for the log.
    Refused(String),
}

/// Decides the transport from size and count.
///
/// A pure, tested function because it is this module's business rule: it
/// is what says "this is a copy" or "this is an accident", and erring on
/// the permissive side costs someone else's disk and bandwidth.
pub fn choose_transport(total: u64, files: usize) -> Transport {
    if files == 0 {
        return Transport::Refused("no file in the selection".into());
    }
    if files > FILE_COUNT_CAP {
        return Transport::Refused(format!(
            "{files} files, above the cap of {FILE_COUNT_CAP}"
        ));
    }
    if total > TOTAL_CAP {
        return Transport::Refused(format!(
            "{} above the cap of {}",
            human_size(total),
            human_size(TOTAL_CAP)
        ));
    }
    if total <= INLINE_CAP {
        Transport::Inline
    } else {
        Transport::Separate
    }
}

/// Readable size. Only for logs and refusal messages.
pub fn human_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / (K * K))
    } else {
        format!("{:.2} GB", f / (K * K * K))
    }
}

/// What the selection contains, measured.
#[derive(Debug, Default)]
pub struct Measure {
    pub bytes: u64,
    pub files: usize,
    /// Stopped before finishing because it already went over a cap.
    pub overflowed: bool,
}

/// Measures the selection, **stopping early** once it went over a cap.
///
/// Stopping early is not an optimization: without it, measuring a
/// selection we are going to refuse would mean walking the whole tree
/// first. Pointing the clipboard at `/` would take minutes of `stat`
/// before the refusal.
pub fn measure(paths: &[String]) -> Measure {
    let mut m = Measure::default();
    for c in paths {
        measure_one(Path::new(c), &mut m);
        if m.overflowed {
            break;
        }
    }
    m
}

fn measure_one(p: &Path, m: &mut Measure) {
    if m.bytes > TOTAL_CAP || m.files > FILE_COUNT_CAP {
        m.overflowed = true;
        return;
    }
    let Ok(meta) = std::fs::symlink_metadata(p) else { return };
    if meta.is_dir() {
        let Ok(it) = std::fs::read_dir(p) else { return };
        for e in it.flatten() {
            measure_one(&e.path(), m);
            if m.overflowed {
                return;
            }
        }
    } else {
        // A symlink counts as an entry but not as its target: following
        // links would make a 3 KB selection measure the whole disk, and a
        // link cycle would never end.
        m.files += 1;
        m.bytes += meta.len();
    }
}

// --- text/uri-list ---------------------------------------------------
//
// The format Wayland, GTK and Finder speak when the subject is a file on
// the clipboard. One URI per line, CRLF-terminated per RFC 2483, and the
// path percent-encoded.
//
// The encoding is the detail that breaks in practice: names with spaces,
// accents, `#`. `final report.pdf` turning into `final` + `report.pdf`
// is the classic bug of this conversion, and that is why it is pure and
// has tests.

/// Characters that may go raw in a file URI.
///
/// `:` is in because it is a valid `pchar` in RFC 3986 and because it is
/// what Explorer and GTK produce: `file:///C:/Users/...`. Escaping it would
/// give `C%3A`, which works on our round trip but is not what another
/// program expects to find on the clipboard.
///
/// `#` and `?` are NOT in: one starts a fragment and the other a query,
/// and a file called `nota #3.txt` would lose half its name.
fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// Local paths -> `text/uri-list`.
pub fn uris_from_paths(paths: &[String]) -> String {
    let mut s = String::new();
    for c in paths {
        // Three slashes when the path does not start with `/`.
        //
        // `file://C:/Users/x` puts `C:` in the URI's AUTHORITY -- that is,
        // it declares a host called `C` on an empty port -- and the path
        // becomes `/Users/x`. The file vanishes and the error does not
        // look like a path error. The right form is `file:///C:/Users/x`.
        s.push_str(if c.starts_with('/') { "file://" } else { "file:///" });
        for &b in c.as_bytes() {
            if unreserved(b) {
                s.push(b as char);
            } else if b == b'\\' {
                // A Windows path becomes a URI with forward slashes,
                // otherwise the destination gets `C:%5CUsers` and finds
                // nothing.
                s.push('/');
            } else {
                s.push_str(&format!("%{b:02X}"));
            }
        }
        s.push_str("\r\n");
    }
    s
}

/// `text/uri-list` -> local paths.
///
/// Ignores blank lines and comments (`#`, per the RFC) and drops any
/// scheme that is not `file://` -- an `http://` on the clipboard is a
/// link, not a file, and treating it as a path would create a file called
/// `http:` in the staging folder.
pub fn paths_from_uris(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in s.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some(rest) = l.strip_prefix("file://") else { continue };
        // `file://host/path` exists in the RFC; we only accept an empty
        // host or `localhost`, because a real host would be a file that
        // is not here.
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        if !rest.starts_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        let b = rest.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(x) => {
                        bytes.push(x);
                        i += 3;
                        continue;
                    }
                    // A stray `%` is a literal `%`, not a reason to drop
                    // the whole path.
                    None => {}
                }
            }
            bytes.push(b[i]);
            i += 1;
        }
        if let Ok(c) = String::from_utf8(bytes) {
            v.push(normalize_windows(&c));
        }
    }
    v
}

/// `/C:/Users/x` -> `C:\Users\x`.
///
/// The Windows file URI has an extra slash before the drive letter, and
/// whoever receives it needs the native path to open it.
fn normalize_windows(c: &str) -> String {
    let b = c.as_bytes();
    if b.len() >= 4 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return c[1..].replace('/', "\\");
    }
    c.to_string()
}

// --- x-special/gnome-copied-files ------------------------------------

/// What Nautilus (and GTK in general) reads when pasting.
///
/// First line `copy` or `cut`, then the URIs. **Always `copy`**, never
/// `cut`: `cut` tells the destination to delete the source after pasting,
/// and here the source is on another machine. We have no way of knowing
/// the paste happened, and deleting someone's file by mistake has no undo.
pub fn gnome_copied_files(paths: &[String]) -> String {
    format!("copy\n{}", uris_from_paths(paths).replace("\r\n", "\n"))
}

// --- staging folder --------------------------------------------------

/// Where received files wait for the paste.
///
/// Outside `/tmp` on purpose: `/tmp` is wiped at boot on some distros and
/// lives in tmpfs (RAM) on others, and 2 GB of clipboard in RAM is a way to
/// meet the OOM killer.
pub fn staging_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
        Path::new(&base).join("oxdesk").join("clipboard")
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Path::new(&base).join(".cache").join("oxdesk").join("clipboard")
    }
}

/// How many deliveries we keep before deleting the old ones.
pub const KEPT_DELIVERIES: usize = 5;

/// Deletes old deliveries, keeping the last [`KEPT_DELIVERIES`].
///
/// Without this the folder grows forever: every file Ctrl+C leaves a copy,
/// and nobody goes there to clean up.
pub fn prune(dir: &Path) {
    let Ok(it) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = it
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    // The name is the delivery's millisecond, so alphabetical order of a
    // zero-padded number is chronological order.
    dirs.sort();
    let excess = dirs.len().saturating_sub(KEPT_DELIVERIES);
    for d in dirs.into_iter().take(excess) {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Delivery name: milliseconds, zero-padded so it sorts.
pub fn delivery_name(now_ms: u128) -> String {
    format!("{now_ms:014}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_separates_copy_from_accident() {
        assert_eq!(choose_transport(1024, 1), Transport::Inline);
        assert_eq!(choose_transport(INLINE_CAP, 3), Transport::Inline);
        assert_eq!(choose_transport(INLINE_CAP + 1, 3), Transport::Separate);
        assert!(matches!(
            choose_transport(TOTAL_CAP + 1, 3),
            Transport::Refused(_)
        ));
        // Large and light tree: passes the byte cap and is blocked by the
        // count. The `node_modules` case.
        assert!(matches!(
            choose_transport(300 * 1024 * 1024, 40_000),
            Transport::Refused(_)
        ));
        assert!(matches!(choose_transport(0, 0), Transport::Refused(_)));
    }

    #[test]
    fn the_refusal_gives_the_reason_in_numbers() {
        // A refusal message without numbers makes the person guess what
        // to reduce.
        let Transport::Refused(m) = choose_transport(3 * 1024 * 1024 * 1024, 2) else {
            panic!("should refuse");
        };
        assert!(m.contains("3.00 GB"), "{m}");
        assert!(m.contains("2.00 GB"), "{m}");
    }

    #[test]
    fn space_and_accent_survive_the_round_trip() {
        // The classic bug: "final report.pdf" turning into two paths.
        let orig = vec![
            "/home/alice/final report.pdf".to_string(),
            "/home/alice/relatório #3.txt".to_string(),
        ];
        let list = uris_from_paths(&orig);
        assert!(list.contains("final%20report.pdf"), "{list}");
        assert!(!list.contains('#'), "the # has to be escaped: {list}");
        assert_eq!(paths_from_uris(&list), orig);
    }

    #[test]
    fn windows_path_round_trips() {
        let w = vec![r"C:\Users\Alice\note.txt".to_string()];
        let list = uris_from_paths(&w);
        assert_eq!(list.trim_end(), "file:///C:/Users/Alice/note.txt");
        assert_eq!(paths_from_uris(&list), w, "round trip on Windows");
        // On the way back, the URI's `/C:/` has to become a native `C:\`.
        assert_eq!(paths_from_uris("file:///C:/Users/x/a.txt"), vec![r"C:\Users\x\a.txt"]);
    }

    #[test]
    fn drops_what_is_not_a_file() {
        let s = "# comentario\r\nhttp://exemplo.com/a.pdf\r\n\r\nfile:///tmp/ok.txt\r\n";
        assert_eq!(paths_from_uris(s), vec!["/tmp/ok.txt"]);
    }

    #[test]
    fn accepts_localhost_and_refuses_a_real_host() {
        assert_eq!(paths_from_uris("file://localhost/tmp/a"), vec!["/tmp/a"]);
        // A file on another machine is not a file here.
        assert!(paths_from_uris("file://outra-maquina/tmp/a").is_empty());
    }

    #[test]
    fn stray_percent_does_not_drop_the_path() {
        assert_eq!(paths_from_uris("file:///tmp/100%/a.txt"), vec!["/tmp/100%/a.txt"]);
    }

    #[test]
    fn never_sends_cut() {
        // `cut` would make the destination delete the source, which is on
        // another machine. There is no undo for that.
        let s = gnome_copied_files(&["/tmp/a".to_string()]);
        assert!(s.starts_with("copy\n"), "{s}");
        assert!(!s.contains("cut"), "{s}");
    }

    #[test]
    fn delivery_sorts_chronologically_as_text() {
        // `prune` relies on this: alphabetical order == time order.
        let mut v = [delivery_name(1762300000000), delivery_name(999), delivery_name(1762300000001)];
        v.sort();
        assert_eq!(v[0], delivery_name(999));
        assert_eq!(v[2], delivery_name(1762300000001));
    }

    #[test]
    fn measure_stops_early_on_a_large_tree() {
        // Do not walk what we are going to refuse. A shallow synthetic
        // tree with more files than the cap.
        let t = std::env::temp_dir().join(format!("oxdesk-measure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        for i in 0..(FILE_COUNT_CAP + 50) {
            std::fs::write(t.join(format!("f{i}")), b"x").unwrap();
        }
        let m = measure(&[t.to_string_lossy().to_string()]);
        assert!(m.overflowed, "should have stopped early");
        assert!(m.files <= FILE_COUNT_CAP + 2, "stopped late: {}", m.files);
        let _ = std::fs::remove_dir_all(&t);
    }
}

// --- clipboard probe (Linux) -----------------------------------------

/// Walks down a `RefArg` collecting every string it finds.
///
/// `as_iter` on a `Variant` goes down **one** level, and an `as` inside a
/// variant is two levels below -- that is why the first probe printed an
/// empty list and made me doubt Mutter instead of my code.
#[cfg(target_os = "linux")]
fn collect_strings(a: &dyn dbus::arg::RefArg, out: &mut Vec<String>) {
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

/// `oxdesk clip-sonda`: shows what Mutter offers on each copy.
///
/// It exists because the question that decides the Linux-side design is
/// not answered by reading documentation: does the `SelectionOwnerChanged`
/// signal say which MIME types the owner published? And does asking for
/// `text/uri-list` when the owner only has text return a clean error, or
/// hang?
///
/// Run it, copy a text, copy a file in Nautilus, compare.
#[cfg(target_os = "linux")]
pub fn cli_probe() -> std::process::ExitCode {
    use dbus::arg::{RefArg, Variant};
    use dbus::blocking::Connection;
    use dbus::Path;
    use std::collections::HashMap;
    use std::time::Duration;

    const RD: &str = "org.gnome.Mutter.RemoteDesktop";
    const RD_OBJ: &str = "/org/gnome/Mutter/RemoteDesktop";
    const RDS: &str = "org.gnome.Mutter.RemoteDesktop.Session";
    let t = Duration::from_secs(5);

    let r = (|| -> Result<(), Box<dyn std::error::Error>> {
        let c = Connection::new_session()?;
        let (sess,): (Path,) = c.with_proxy(RD, RD_OBJ, t).method_call(RD, "CreateSession", ())?;
        c.with_proxy(RD, sess.clone(), t).method_call::<(), _, _, _>(RDS, "Start", ())?;
        let empty: HashMap<&str, Variant<Box<dyn RefArg>>> = HashMap::new();
        c.with_proxy(RD, sess.clone(), t)
            .method_call::<(), _, _, _>(RDS, "EnableClipboard", (empty,))?;
        println!("watching (session {sess}). copy something; ctrl-c to quit.\n");

        let rule = dbus::message::MatchRule::new_signal(RDS, "SelectionOwnerChanged")
            .with_path(sess.clone());
        c.add_match(rule, |_: (), _, _| true)?;

        loop {
            let Some(m) = c.channel().blocking_pop_message(Duration::from_millis(400))? else {
                continue;
            };
            if m.member().map(|s| s.to_string()).as_deref() != Some("SelectionOwnerChanged") {
                continue;
            }
            // The signal's arguments, as they come.
            match m.read1::<HashMap<String, Variant<Box<dyn RefArg>>>>() {
                Ok(opts) => {
                    let keys: Vec<&String> = opts.keys().collect();
                    println!("signal with keys {keys:?}");
                    if let Some(v) = opts.get("mime-types") {
                        println!("  signature: {}", v.0.signature());
                        let mut types = Vec::new();
                        collect_strings(&*v.0, &mut types);
                        println!("  mime-types: {types:?}");
                    }
                    if let Some(v) = opts.get("session-is-owner") {
                        println!("  session-is-owner: {:?}", v.0.as_u64().map(|x| x == 1));
                    }
                }
                Err(e) => println!("signal without a readable dict ({e})"),
            }
            // The other half of the question: always ask for uri-list.
            let now = std::time::Instant::now();
            let r: Result<(dbus::arg::OwnedFd,), _> = c
                .with_proxy(RD, sess.clone(), t)
                .method_call(RDS, "SelectionRead", ("text/uri-list",));
            match r {
                Ok(_) => println!("  SelectionRead(uri-list): OK in {:?}\n", now.elapsed()),
                Err(e) => println!(
                    "  SelectionRead(uri-list): error in {:?} -- {}\n",
                    now.elapsed(),
                    e.message().unwrap_or("no message")
                ),
            }
        }
    })();
    if let Err(e) = r {
        eprintln!("clip-sonda: {e}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

// --- echo ------------------------------------------------------------

/// Last list that went through here, in either direction.
static LAST: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// Records the list and says whether it is NEW.
///
/// Same role as [`crate::clip::record`]: it is the `false` that cuts the
/// loop of A sends to B, B sees it change and sends it back.
///
/// Compares by **file name**, not full path: the destination writes
/// `~/.cache/oxdesk/clipboard/.../nota.txt` while the source has
/// `~/dev/nota.txt`, and comparing paths would make every receipt look
/// like a new copy -- that is, an endless echo between three machines.
pub fn record(paths: &[String]) -> bool {
    let key: Vec<String> = paths.iter().map(|c| base_name(c)).collect();
    let Ok(mut g) = LAST.lock() else { return false };
    if g.as_deref() == Some(&key[..]) {
        return false;
    }
    *g = Some(key);
    true
}

/// Last component of the path, with either world's separator.
pub fn base_name(c: &str) -> String {
    c.rsplit(['/', '\\']).next().unwrap_or(c).to_string()
}

/// Directory containing the path. Empty if there is none.
pub fn parent_of(c: &str) -> String {
    match c.rfind(['/', '\\']) {
        Some(0) => "/".to_string(),
        Some(i) => c[..i].to_string(),
        None => String::new(),
    }
}

// --- reading the local clipboard (Windows and macOS) -----------------
//
// Linux does not go through here: there the read is born from the Mutter
// signal, inside the thread that owns the D-Bus session.

/// JXA script that READS the file URLs from NSPasteboard.
///
/// JXA and not AppleScript because `set the clipboard to {POSIX file ...}`
/// publishes an opaque type (`'list'`) and **not** `public.file-url` --
/// measured: Finder would not paste it. `readObjectsForClasses` is the
/// right API, and `isFileURL` filters the case of a copied `http` link,
/// which would otherwise become a "file" called `https:`.
///
/// Returns BOTH things in one call -- `A\n<paths>` or `T\n<text>` --
/// because the cost here is the process, not the work: measured, 55 ms
/// per `osascript` against 11 ms for `pbpaste`. Every 2 s, running both
/// would be 3.3% of a core forever on a laptop. With the unified read
/// macOS spends ONE process per sample, less than it spent before this
/// feature existed.
#[cfg(target_os = "macos")]
const READ_PB_JS: &str = r#"ObjC.import("AppKit");const pb=$.NSPasteboard.generalPasteboard;const o=pb.readObjectsForClassesOptions($([$.NSURL]),$({}));const r=[];if(!o.isNil()){for(let i=0;i<o.count;i++){const u=o.objectAtIndex(i);if(u.isFileURL)r.push(u.path.js)}}r.length?"A\n"+r.join("\n"):"T\n"+($.NSString.stringWithString(pb.stringForType($.NSPasteboardTypeString)||"").js)"#;

/// JXA script that WRITES file URLs to NSPasteboard.
///
/// The paths come through **argv**, not interpolated into the script:
/// file names carry quotes, slashes and accents, and building JavaScript
/// by string concatenation is how this kind of code breaks.
#[cfg(target_os = "macos")]
const WRITE_PB_JS: &str = r#"function run(argv){ObjC.import("AppKit");const pb=$.NSPasteboard.generalPasteboard;pb.clearContents;const u=argv.map(p=>$.NSURL.fileURLWithPath(p));return pb.writeObjects($(u))?"ok":"failed"}"#;

/// Writes a script to disk for `osascript` to read.
///
/// In a file and not via `-e` because the script has `function run(argv)`
/// and double quotes all over; passing that on the command line invites
/// quoting bugs -- the project already lost time to three of them in the
/// PowerShell bridge.
#[cfg(target_os = "macos")]
fn script_on_disk(name: &str, body: &str) -> Result<PathBuf, String> {
    // In the PARENT of the staging folder: inside it, the script would
    // show up next to the deliveries and `entries()` would send it to the
    // clipboard.
    let dir = staging_dir().parent().map(|p| p.to_path_buf()).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let p = dir.join(name);
    std::fs::write(&p, body).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

/// The files on THIS machine's clipboard, if any.
#[cfg(windows)]
pub fn read_local() -> Vec<String> {
    winput::clipboard_read_files().ok().flatten().unwrap_or_default()
}

/// What is on the Mac's clipboard: files, text, or nothing.
#[cfg(target_os = "macos")]
pub enum Clipboard {
    Files(Vec<String>),
    Text(String),
    Empty,
}

#[cfg(target_os = "macos")]
pub fn read_clipboard() -> Clipboard {
    // Every failure path **notes it** before returning `Empty`.
    //
    // Without that "the clipboard is empty" and "osascript does not run"
    // leave here with the same value, and the second case -- which is
    // what happens when a permission is missing or the script could not
    // be written -- would become a silent no-op. Whoever reads the note is
    // the setup wizard, so the notes stay pt-BR.
    let js = match script_on_disk("ler-pb.js", READ_PB_JS) {
        Ok(js) => js,
        Err(e) => {
            crate::clip::note_watch(false, format!("não pude escrever o script de leitura: {e}"));
            return Clipboard::Empty;
        }
    };
    let o = match crate::sys::child("osascript").args(["-l", "JavaScript"]).arg(&js).output() {
        Ok(o) => o,
        Err(e) => {
            crate::clip::note_watch(false, format!("osascript não executou: {e}"));
            return Clipboard::Empty;
        }
    };
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("sem mensagem");
        crate::clip::note_watch(false, format!("osascript recusou: {line}"));
        return Clipboard::Empty;
    }
    crate::clip::note_watch(true, "osascript lê a área de transferência");
    let out = String::from_utf8_lossy(&o.stdout);
    let mut lines = out.lines();
    match lines.next() {
        Some("A") => {
            let v: Vec<String> = lines
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            if v.is_empty() {
                Clipboard::Empty
            } else {
                Clipboard::Files(v)
            }
        }
        // The text may have line breaks, so join the rest back instead of
        // taking only the first.
        Some("T") => {
            let t: Vec<&str> = lines.collect();
            let t = t.join("\n");
            if t.trim().is_empty() {
                Clipboard::Empty
            } else {
                Clipboard::Text(t)
            }
        }
        _ => Clipboard::Empty,
    }
}

#[cfg(target_os = "macos")]
pub fn read_local() -> Vec<String> {
    match read_clipboard() {
        Clipboard::Files(v) => v,
        _ => Vec::new(),
    }
}

#[cfg(target_os = "linux")]
pub fn read_local() -> Vec<String> {
    Vec::new()
}

/// Puts the files on THIS machine's clipboard.
#[cfg(windows)]
pub fn write_local(paths: &[String]) -> Result<(), String> {
    winput::clipboard_write_files(paths)
}

#[cfg(target_os = "macos")]
pub fn write_local(paths: &[String]) -> Result<(), String> {
    let js = script_on_disk("escrever-pb.js", WRITE_PB_JS)?;
    let o = crate::sys::child("osascript")
        .args(["-l", "JavaScript"])
        .arg(&js)
        .args(paths)
        .output()
        .map_err(|e| format!("osascript: {e}"))?;
    if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ok") {
        Ok(())
    } else {
        Err(format!(
            "osascript refused: {}{}",
            String::from_utf8_lossy(&o.stdout).trim(),
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

// --- pushing to the peers --------------------------------------------

/// Large selection waiting for a hand-picked destination.
///
/// Above [`INLINE_CAP`] we do **not** send to everyone: copying 1.4 GB to
/// two machines you are not looking at wastes someone else's bandwidth
/// and disk for nothing. The selection is recorded here and the tray
/// offers "send to <host>", one destination at a time.
static OFFER: std::sync::Mutex<Option<(Vec<String>, u64)>> = std::sync::Mutex::new(None);

/// The pending large selection, if any: (paths, bytes).
pub fn offer() -> Option<(Vec<String>, u64)> {
    OFFER.lock().ok().and_then(|g| g.clone())
}

struct Peer {
    name: String,
    ssh: String,
    /// The host, kept to build the remote command at the time.
    cfg: crate::config::HostCfg,
    windows: bool,
}

fn peers(cfg: &crate::config::Config) -> Vec<Peer> {
    cfg.hosts
        .iter()
        .filter(|h| {
            !h.local.unwrap_or(false)
                && !matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1")
        })
        .filter_map(|h| {
            h.ssh.clone().map(|s| Peer {
                name: h.name.clone(),
                ssh: s,
                cfg: h.clone(),
                windows: h.os.as_deref() == Some("windows"),
            })
        })
        .collect()
}

/// Called when someone copied files on THIS machine.
///
/// Decides between sending right away, offering, or refusing -- and
/// **says** which of the three in the log. A copy that silently goes
/// nowhere is worse than an explained refusal: you find out at paste time,
/// on the other side, with no clue why.
pub fn on_copy(cfg: &crate::config::Config, paths: &[String]) {
    if !record(paths) {
        return; // it is what we already have; sending again is echo
    }
    // Two files with the same base name would land on top of each other
    // in the staging folder, and the person would paste one file thinking
    // there were two. Refusing is honest; silently renaming is not.
    let mut bases: Vec<String> = paths.iter().map(|c| base_name(c)).collect();
    bases.sort();
    let before = bases.len();
    bases.dedup();
    if bases.len() != before {
        crate::say!("clip-arq: two files with the same name in the selection; not sent");
        return;
    }

    let m = measure(paths);
    match choose_transport(m.bytes, m.files) {
        Transport::Refused(reason) => {
            if let Ok(mut g) = OFFER.lock() {
                *g = None;
            }
            crate::say!("clip-arq: {reason}; not sent");
        }
        Transport::Inline => {
            if let Ok(mut g) = OFFER.lock() {
                *g = None;
            }
            let n = m.files;
            crate::say!("clip-arq: sending {n} file(s), {}", human_size(m.bytes));
            for p in peers(cfg) {
                let c = paths.to_vec();
                std::thread::spawn(move || {
                    match send_tar(&p, &c) {
                        Ok(()) => crate::say!("clip-arq: {} received", p.name),
                        Err(e) => crate::say!("clip-arq: {} did not receive -- {e}", p.name),
                    }
                });
            }
        }
        Transport::Separate => {
            if let Ok(mut g) = OFFER.lock() {
                *g = Some((paths.to_vec(), m.bytes));
            }
            crate::say!(
                "clip-arq: {} in {} file(s) -- too large to go on its own; pick the destination in the tray",
                human_size(m.bytes),
                m.files
            );
        }
    }
}

/// Builds the tar's `-C <parent> <base>` pairs.
///
/// One `-C` per file because the selection may come from different
/// folders, and `tar` accepts several (tested on GNU tar and on Windows'
/// bsdtar). The file goes into the archive by base name, so the
/// destination tree does not reproduce `/home/alice/dev/...` -- the
/// staging folder stays flat and pasteable.
pub fn tar_args(paths: &[String]) -> Vec<String> {
    let mut v = Vec::new();
    for c in paths {
        let parent = parent_of(c);
        if parent.is_empty() {
            continue;
        }
        v.push("-C".into());
        v.push(parent);
        v.push(base_name(c));
    }
    v
}

/// `tar -cf - ... | ssh peer oxdesk clip-arq-set`
///
/// The receiver **does not get the list of names**. After unpacking, the
/// top-level entries of the new folder *are* the selection -- and that
/// avoids passing file names with spaces and accents through a remote
/// shell that, on Windows, is PowerShell. Three quoting bugs on that
/// bridge already cost time (specs/002).
fn send_tar(p: &Peer, paths: &[String]) -> Result<(), String> {
    use std::process::Stdio;
    let mut tar = crate::sys::child("tar")
        .arg("-cf")
        .arg("-")
        .args(tar_args(paths))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("tar: {e}"))?;
    let out = tar.stdout.take().ok_or("tar without stdout")?;
    let ssh = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", &p.ssh])
        .arg(crate::remote::oxdesk_command(&p.cfg, "clip-arq-set"))
        .stdin(Stdio::from(out))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ssh: {e}"))?;
    let o = ssh.wait_with_output().map_err(|e| format!("ssh: {e}"))?;
    let _ = tar.wait();
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {}",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

/// Sends the pending selection to ONE host. It is what the tray calls.
pub fn send_offer(cfg: &crate::config::Config, host: &str) -> Result<String, String> {
    let (paths, bytes) = offer().ok_or("no large selection pending")?;
    let p = peers(cfg)
        .into_iter()
        .find(|p| p.name == host)
        .ok_or_else(|| format!("host '{host}' is not in the config or has no ssh"))?;
    // `rsync` only where it exists on both ends -- Windows has `tar.exe`
    // and `scp.exe` in `system32`, but no rsync (measured). The
    // distinction is worth it because this is where `--partial` pays off:
    // on a Wi-Fi drop in the middle of 2 GB, `tar` loses the whole
    // transfer and `rsync` only the rest.
    let use_rsync = !p.windows && !cfg!(windows);
    let r = if use_rsync {
        send_rsync(&p, &paths)
    } else {
        send_tar(&p, &paths)
    };
    match r {
        Ok(()) => {
            let via = if use_rsync { "rsync" } else { "tar" };
            crate::say!("clip-arq: {} went to {} via {via}", human_size(bytes), p.name);
            Ok(format!("{} sent to {}", human_size(bytes), p.name))
        }
        Err(e) => Err(format!("{}: {e}", p.name)),
    }
}

/// rsync with resume, in three steps.
///
/// It needs all three because rsync does not create the nested
/// destination folder: the peer opens the folder and says the path, rsync
/// fills it, and a third SSH says it arrived. That is three handshakes
/// against one for `tar`, and it only pays off on a large transfer --
/// which is exactly where this function is used.
fn send_rsync(p: &Peer, paths: &[String]) -> Result<(), String> {
    let dest = &p.ssh;
    let o = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", dest])
        .arg(crate::remote::oxdesk_command(&p.cfg, "clip-arq-dir"))
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    if !o.status.success() {
        return Err(format!("clip-arq-dir: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let dir = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if dir.is_empty() {
        return Err("the peer did not answer with the folder path".into());
    }

    let o = crate::sys::child("rsync")
        .args(["-a", "--partial", "--append-verify"])
        .args(paths)
        .arg(format!("{dest}:{dir}/"))
        .output()
        .map_err(|e| format!("rsync: {e}"))?;
    if !o.status.success() {
        return Err(format!("rsync: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }

    let o = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", dest])
        .arg(crate::remote::oxdesk_command(&p.cfg, &format!("clip-arq-prontos {dir}")))
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("clip-arq-prontos: {}", String::from_utf8_lossy(&o.stderr).trim()))
    }
}

// --- RECEIVING side --------------------------------------------------

/// Opens a new delivery folder and prints its path.
///
/// `oxdesk clip-arq-dir`, called by the source's rsync before copying.
pub fn cli_dir() -> std::process::ExitCode {
    match new_delivery() {
        Ok(d) => {
            println!("{}", d.display());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("clip-arq-dir: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn new_delivery() -> Result<PathBuf, String> {
    let base = staging_dir();
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    prune(&base);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let d = base.join(delivery_name(ms));
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    Ok(d)
}

/// What was left in the delivery folder, as absolute paths.
///
/// This is what replaces sending the list of names on the command line:
/// after the `tar`, whatever is at the folder's top level *is* the
/// selection. A name with spaces, accents or quotes never goes through a
/// shell.
fn entries(dir: &Path) -> Vec<String> {
    let Ok(it) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<String> = it
        .flatten()
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

/// `oxdesk clip-arq-set`: unpacks the tar from stdin and puts it on the clipboard.
///
/// Reads stdin **in this process** and writes a file, instead of letting
/// `tar` inherit SSH's stdin. The `Stdio::inherit` version looked better
/// -- one buffer fewer -- and on Windows delivered an empty folder with
/// exit 0, no message at all: `oxdesk.exe` is `windows_subsystem =
/// "windows"` (no console), and in that condition the child did not get
/// the stdin the parent process receives without trouble. The text
/// `clip-set` works precisely because it reads stdin inside the process --
/// so that is the proven path, and it is the one we use.
///
/// Keeping it all in memory is safe because this path only exists below
/// [`INLINE_CAP`]; above it the transfer is separate. The 1 MiB slack
/// covers the tar header and block padding.
pub fn cli_set(port: u16) -> std::process::ExitCode {
    use std::io::Read;
    let cap = INLINE_CAP + 1024 * 1024;
    let mut archive = Vec::new();
    // `take`: a stream larger than the cap stops here and not in RAM.
    if let Err(e) = std::io::stdin().take(cap + 1).read_to_end(&mut archive) {
        eprintln!("clip-arq-set: could not read stdin: {e}");
        return std::process::ExitCode::FAILURE;
    }
    if archive.is_empty() {
        eprintln!("clip-arq-set: empty stdin -- no archive arrived");
        return std::process::ExitCode::FAILURE;
    }
    if archive.len() as u64 > cap {
        eprintln!("clip-arq-set: archive above {}", human_size(cap));
        return std::process::ExitCode::FAILURE;
    }
    let dir = match new_delivery() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("clip-arq-set: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Outside the delivery folder: inside, the .tar itself would show up
    // in `entries()` and go to the clipboard along with the files.
    let tar_path = staging_dir().join(format!("{}.tar", delivery_name(0)));
    if let Err(e) = std::fs::write(&tar_path, &archive) {
        eprintln!("clip-arq-set: {}: {e}", tar_path.display());
        return std::process::ExitCode::FAILURE;
    }
    let r = crate::sys::child("tar").arg("-xf").arg(&tar_path).arg("-C").arg(&dir).output();
    let _ = std::fs::remove_file(&tar_path);
    match r {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!("clip-arq-set: tar failed -- {}", String::from_utf8_lossy(&o.stderr).trim());
            return std::process::ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("clip-arq-set: tar: {e}");
            return std::process::ExitCode::FAILURE;
        }
    }
    deliver(port, &dir)
}

/// `oxdesk clip-arq-prontos <dir>`: the files have already arrived (rsync).
pub fn cli_ready(port: u16, dir: &str) -> std::process::ExitCode {
    let d = PathBuf::from(dir);
    // Only inside the staging folder. Without this, a `clip-arq-prontos /`
    // would point the clipboard at the root -- and the caller comes from
    // the network.
    if !d.starts_with(staging_dir()) {
        eprintln!("clip-arq-prontos: {dir} is outside the staging folder");
        return std::process::ExitCode::FAILURE;
    }
    deliver(port, &d)
}

/// Hands the paths to the local daemon, which is what touches the clipboard.
///
/// Through the daemon and not directly, for the same reason as
/// [`crate::clip::cli_set`]: on Linux whoever writes the selection has to
/// STAY ALIVE to answer `SelectionTransfer` at paste time. A CLI process
/// that writes and exits leaves a selection with no owner, and Ctrl+V finds
/// nothing.
fn deliver(port: u16, dir: &Path) -> std::process::ExitCode {
    let items = entries(dir);
    if items.is_empty() {
        eprintln!("clip-arq: nothing arrived in {}", dir.display());
        return std::process::ExitCode::FAILURE;
    }
    let body = serde_json::json!({ "paths": items }).to_string().into_bytes();
    match post(port, "/api/clipboard-files", &body) {
        Ok(()) => {
            println!("{} file(s) on the clipboard", items.len());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("clip-arq: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Raw POST to localhost. Twelve lines against a whole HTTP client.
fn post(port: u16, route: &str, body: &[u8]) -> Result<(), String> {
    use std::io::{Read, Write};
    let request = format!(
        "POST {route} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("the local oxdesk did not answer on port {port}: {e}"))?;
    s.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(body).map_err(|e| e.to_string())?;
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    if resp.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(resp.lines().next().unwrap_or("no response").to_string())
    }
}

/// Writes to the local clipboard what arrived from a peer.
pub fn apply_local(paths: &[String]) -> Result<(), String> {
    if !record(paths) {
        return Ok(()); // it already is that; writing again would only echo
    }
    #[cfg(target_os = "linux")]
    {
        crate::clip::publish_files(paths)
    }
    #[cfg(not(target_os = "linux"))]
    {
        write_local(paths)
    }
}
