//! Bridge between a screen host and a panel tab: QUIC on one side, a local
//! WebSocket on the other. See specs/016-session-and-visuals.md.
//!
//!   LOGI_PONTE_TOKEN=<token> logistream-ponte <host:port>
//!
//! Prints `porta <n>` once it listens. The token comes through the
//! environment because argv shows up in `ps`.
//!
//! The socket is on 127.0.0.1, and that is not protection by itself: any page
//! open in the browser can try a local WebSocket, and whoever gets through
//! types on the remote machine. So it takes the token AND the panel's Origin.
//!
//! Video goes to the page as `[flags u8][captured_at_ns u64 BE][Annex B]`.
//! Input is the wire format of `Event`, forwarded without being interpreted.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use logistream::net::{self, Reassembler};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

/// Nobody connected in this long: the tab is gone, and so is the reason to run.
const TAB_WAIT: Duration = Duration::from_secs(30);
/// Largest `Event` on the wire: a tag byte and two `f64`.
const MAX_EVENT: usize = 17;
const DEFAULT_ORIGIN: &str = "http://127.0.0.1:8787";

/// Does this request come from the panel, with this session's token?
fn authorized(path: &str, origin: Option<&str>, token: &str, expected: &str) -> bool {
    !token.is_empty()
        && path.strip_prefix('/') == Some(token)
        && origin == Some(expected)
}

/// Frame header for the page: bit 0 of `flags` marks a keyframe.
fn wrap(keyframe: bool, captured_at_ns: u64, unit: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(9 + unit.len());
    m.push(u8::from(keyframe));
    m.extend_from_slice(&captured_at_ns.to_be_bytes());
    m.extend_from_slice(unit);
    m
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let target = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: logistream-ponte <host:port>"))?;
    let token = std::env::var("LOGI_PONTE_TOKEN").unwrap_or_default();
    anyhow::ensure!(!token.is_empty(), "LOGI_PONTE_TOKEN is missing");
    let origin = std::env::var("LOGI_PONTE_ORIGEM").unwrap_or_else(|_| DEFAULT_ORIGIN.into());

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    // Parsed by logid (`screen.rs`): keep it byte-identical.
    println!("porta {}", listener.local_addr()?.port());

    let deadline = tokio::time::Instant::now() + TAB_WAIT;

    let gate = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let from = request.headers().get("origin").and_then(|v| v.to_str().ok());
        if authorized(request.uri().path(), from, &token, &origin) {
            return Ok(response);
        }
        eprintln!("refused a connection: wrong token or origin");
        let mut denied = ErrorResponse::new(None);
        *denied.status_mut() = StatusCode::FORBIDDEN;
        Err(denied)
    };
    // Refusing does not end the wait: otherwise any page could kill the
    // session just by trying, before the tab arrives.
    let mut ws = loop {
        let (tcp, _) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| anyhow::anyhow!("no tab connected"))??;
        if let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, gate).await {
            break ws;
        }
    };
    drop(listener);

    let mut addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&target).await?.collect();
    addrs.sort_by_key(|a| !a.is_ipv4());
    let dest = *addrs.first().ok_or_else(|| anyhow::anyhow!("could not resolve {target}"))?;
    let bind: std::net::SocketAddr =
        if dest.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    // Telemetry (spec 017): on only when logid passes the file.
    let telemetry_file = std::env::var_os("LOGI_TELEMETRY_FILE").map(std::path::PathBuf::from);
    let mut stats = logistream::telemetry::Session::new(
        std::env::var("LOGI_VERSION").unwrap_or_default(),
        std::env::var("LOGI_HOST_OS").unwrap_or_default(),
        std::env::consts::OS.into(),
        std::env::var("LOGI_PATH").unwrap_or_else(|_| "unknown".into()),
    );
    let started = std::time::Instant::now();
    let ep = net::client(bind)?;
    let conn = match ep.connect(dest, "logistream")?.await {
        Ok(c) => c,
        Err(e) => {
            if let Some(f) = &telemetry_file {
                stats.append(f, logistream::telemetry::End::Refused, started.elapsed());
            }
            return Err(e.into());
        }
    };
    let started = std::time::Instant::now();
    let end = run(&conn, &mut ws, &mut stats).await;
    if let Some(f) = &telemetry_file {
        stats.append(f, end, started.elapsed());
    }
    Ok(())
}

/// The session itself. Returns how it ended, for telemetry; every exit path
/// goes through here so no session goes unrecorded.
async fn run(
    conn: &quinn::Connection,
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    stats: &mut logistream::telemetry::Session,
) -> logistream::telemetry::End {
    use logistream::telemetry::End;
    // stderr, not stdout: whoever launched us reads only the port and closes
    // the pipe, and a `println!` on a closed pipe kills the process mid-session.
    eprintln!("connected to {}", conn.remote_address());

    let (tx_in, mut rx_in) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let conn_in = conn.clone();
    tokio::spawn(async move {
        let Ok(mut send) = conn_in.open_uni().await else { return };
        while let Some(event) = rx_in.recv().await {
            if send.write_all(&event).await.is_err() {
                break;
            }
        }
    });

    let mut r = Reassembler::default();
    // Asks for a keyframe while there is nothing drawable (net::KeyframeWatch).
    let mut watch = net::KeyframeWatch::new(std::time::Instant::now());
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    let mut ticks = 0u64;
    loop {
        tokio::select! {
            d = conn.read_datagram() => {
                let Ok(d) = d else { return End::Dropped };
                let pushed = r.push(&d);
                stats.frames_lost = r.lost;
                watch.losses(r.lost, std::time::Instant::now());
                let Some((header, unit)) = pushed else { continue };
                stats.frames += 1;
                stats.bytes += unit.len() as u64;
                watch.frame(header.keyframe);
                let m = wrap(header.keyframe, header.captured_at_ns, &unit);
                if ws.send(Message::Binary(m.into())).await.is_err() {
                    return End::Closed;
                }
            }
            _ = tick.tick() => {
                if watch.should_request(std::time::Instant::now()) {
                    stats.keyframe_requests += 1;
                    let _ = conn.send_datagram(bytes::Bytes::from_static(net::KEYFRAME_REQUEST));
                }
                ticks += 1;
                if ticks % 10 == 0 {
                    stats.rtt_sample(conn.rtt());
                }
            }
            m = ws.next() => match m {
                Some(Ok(Message::Binary(b))) if b.len() <= MAX_EVENT => {
                    let _ = tx_in.send(b.to_vec()).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return End::Closed,
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const O: &str = "http://127.0.0.1:8787";

    #[test]
    fn only_the_panel_with_the_token_gets_in() {
        assert!(authorized("/abc", Some(O), "abc", O));
        assert!(!authorized("/wrong", Some(O), "abc", O));
        assert!(!authorized("/abc", Some("https://other.site"), "abc", O));
        assert!(!authorized("/abc", None, "abc", O));
        assert!(!authorized("/", Some(O), "", O));
    }

    #[test]
    fn the_frame_carries_the_keyframe_flag_and_the_instant() {
        let m = wrap(true, 0x0102, &[9, 8]);
        assert_eq!(m, [1, 0, 0, 0, 0, 0, 0, 1, 2, 9, 8]);
        assert_eq!(wrap(false, 0, &[])[0], 0);
    }
}
