//! Checks Mutter's input injection API before building on top of it.
//!
//! `gdbus` does not work here: each call opens its own connection, and
//! Mutter destroys the session when the peer leaves the bus -- the
//! introspect arrived after it had already died.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Linux only");
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    use dbus::blocking::Connection;
    use dbus::Path;
    use std::time::Duration;

    const RD: &str = "org.gnome.Mutter.RemoteDesktop";
    const RD_OBJ: &str = "/org/gnome/Mutter/RemoteDesktop";
    const T: Duration = Duration::from_secs(10);

    let conn = Connection::new_session()?;
    let root = conn.with_proxy(RD, RD_OBJ, T);
    let version: i32 = root.get(RD, "Version").unwrap_or(-1);
    let kinds: u32 = root.get(RD, "SupportedDeviceTypes").unwrap_or(0);
    println!("  RemoteDesktop version {version}, supported devices {kinds:#b}");
    println!("    (1=keyboard 2=mouse 4=touch)");

    let (sess,): (Path,) = root.method_call(RD, "CreateSession", ())?;
    let sess = sess.into_static();
    println!("  session: {sess}");

    let sp = conn.with_proxy(RD, sess.clone(), T);
    let xml: (String,) = sp.method_call("org.freedesktop.DBus.Introspectable", "Introspect", ())?;
    for l in xml.0.lines() {
        let t = l.trim();
        if t.starts_with("<method name=") || t.starts_with("<property ") {
            println!("  {t}");
        } else if t.starts_with("<arg ") {
            println!("      {t}");
        }
    }

    let id: String = sp.get("org.gnome.Mutter.RemoteDesktop.Session", "SessionId").unwrap_or_default();
    println!("  SessionId = {id:?}  (it is what ties the screen session to this one)");
    let _ = sp.method_call::<(), _, _, _>("org.gnome.Mutter.RemoteDesktop.Session", "Stop", ());
    Ok(())
}
