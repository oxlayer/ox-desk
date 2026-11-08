//! Transport: QUIC with datagrams for video, streams for control.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Why QUIC and not raw UDP
//!
//! UDP+FEC is the right shape for video -- a late frame is useless, and
//! retransmitting what already passed is worse than losing it. But
//! writing raw UDP also means writing the handshake, crypto, congestion
//! control and reassembly. QUIC delivers all four **and is UDP**:
//! datagrams are not retransmitted, which is exactly video's semantics.
//!
//! As a bonus, the peer's identity becomes the TLS certificate -- pairing
//! becomes "trust this public key", which is the model the user
//! described: accept once, keep it in the keychain.
//!
//! ## The size limit drives the format
//!
//! A QUIC datagram does not fragment: whatever does not fit the MTU is
//! refused. A 1080p keyframe is tens of KB, so it **has** to be sliced by
//! us -- and that is why the header carries `frame`/`piece`/`pieces`, not
//! a taste for protocols.

use anyhow::{Context, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::net::SocketAddr;
use std::sync::Arc;

/// Header of each slice. 16 bytes, big-endian.
///
/// Fixed and small on purpose: it travels in EVERY datagram, and every
/// byte here comes out of the MTU budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceHeader {
    /// Frame number. Used to drop a slice that arrived late: an old frame
    /// is useless, and assembling it would delay the next one.
    pub frame: u32,
    pub piece: u16,
    pub pieces: u16,
    pub keyframe: bool,
    /// Capture time in nanoseconds, to measure end-to-end latency. Without
    /// it there is no telling whether a change made things better or worse.
    pub captured_at_ns: u64,
}

pub const HEADER_LEN: usize = 17;

impl SliceHeader {
    pub fn write_to(&self, buf: &mut BytesMut) {
        buf.put_u32(self.frame);
        buf.put_u16(self.piece);
        buf.put_u16(self.pieces);
        buf.put_u8(u8::from(self.keyframe));
        buf.put_u64(self.captured_at_ns);
    }

    pub fn read_from(buf: &mut impl Buf) -> Option<Self> {
        if buf.remaining() < HEADER_LEN {
            return None;
        }
        Some(SliceHeader {
            frame: buf.get_u32(),
            piece: buf.get_u16(),
            pieces: buf.get_u16(),
            keyframe: buf.get_u8() != 0,
            captured_at_ns: buf.get_u64(),
        })
    }
}

/// Cuts an encoded packet into slices that fit a datagram.
pub fn slice(packet: &[u8], header: SliceHeader, max_datagram: usize) -> Vec<Bytes> {
    let usable = max_datagram.saturating_sub(HEADER_LEN).max(1);
    let total = packet.len().div_ceil(usable).max(1) as u16;
    packet
        .chunks(usable)
        .enumerate()
        .map(|(i, part)| {
            let mut b = BytesMut::with_capacity(HEADER_LEN + part.len());
            SliceHeader { piece: i as u16, pieces: total, ..header }.write_to(&mut b);
            b.put_slice(part);
            b.freeze()
        })
        .collect()
}

/// The one message a viewer sends on the VIDEO connection: "I cannot draw,
/// send a keyframe". The host reads nothing else from the viewer on this
/// connection (input travels on its own port), so one byte is enough.
pub const KEYFRAME_REQUEST: &[u8] = b"K";

/// When the viewer should ask the host for a keyframe.
///
/// Two ways to end up with nothing to draw, both measured on 2026-11-07:
/// the keyframe that opens the session is lost on the way, or a later frame
/// is lost and every P after it references what never arrived. On a moving
/// screen the next IDR fixes it within the GOP; on a STILL screen the host
/// sends nothing more and the tab stays black for good (Ubuntu, three
/// sessions: 2.7 KB, 71 KB, 363 KB received, the 89 KB IDR sent every time).
pub struct KeyframeWatch {
    synced: bool,
    lost_seen: u64,
    since: std::time::Instant,
    last_request: Option<std::time::Instant>,
}

/// Wait this long without a usable keyframe before asking, and between asks.
pub const KEYFRAME_RETRY: std::time::Duration = std::time::Duration::from_millis(300);

impl KeyframeWatch {
    pub fn new(now: std::time::Instant) -> Self {
        Self { synced: false, lost_seen: 0, since: now, last_request: None }
    }

    /// A whole frame arrived.
    pub fn frame(&mut self, keyframe: bool) {
        if keyframe {
            self.synced = true;
        }
    }

    /// The reassembler's loss counter, after each push. Any new loss breaks
    /// the reference chain until the next keyframe.
    pub fn losses(&mut self, lost: u64, now: std::time::Instant) {
        if lost > self.lost_seen {
            self.lost_seen = lost;
            if self.synced {
                self.synced = false;
                self.since = now;
                // The wait restarts from the loss, not from a request made
                // before the last sync.
                self.last_request = None;
            }
        }
    }

    /// Should a request go out now? Records it when the answer is yes.
    pub fn should_request(&mut self, now: std::time::Instant) -> bool {
        if self.synced {
            return false;
        }
        let from = self.last_request.unwrap_or(self.since);
        if now.duration_since(from) < KEYFRAME_RETRY {
            return false;
        }
        self.last_request = Some(now);
        true
    }
}

/// Reassembles a frame from the slices that arrived.
///
/// Holds one frame at a time: as soon as a slice of a newer frame arrives,
/// the old one is abandoned. Holding several "just in case" would spend
/// memory to deliver old frames, which nobody wants to see.
#[derive(Default)]
pub struct Reassembler {
    frame: u32,
    pieces: Vec<Option<Bytes>>,
    present: usize,
    header: Option<SliceHeader>,
    /// Frames abandoned because they arrived incomplete. This is the
    /// number that says whether turning on FEC is worth it.
    pub lost: u64,
}

impl Reassembler {
    /// Returns the whole frame when its last slice arrives.
    pub fn push(&mut self, datagram: &[u8]) -> Option<(SliceHeader, Vec<u8>)> {
        let mut buf = datagram;
        let header = SliceHeader::read_from(&mut buf)?;
        let body = Bytes::copy_from_slice(buf);

        if header.frame != self.frame || self.pieces.len() != header.pieces as usize {
            if self.present > 0 && self.present < self.pieces.len() {
                self.lost += 1;
            }
            self.frame = header.frame;
            self.pieces = vec![None; header.pieces as usize];
            self.present = 0;
            self.header = Some(header);
        }
        let i = header.piece as usize;
        if i >= self.pieces.len() || self.pieces[i].is_some() {
            return None;
        }
        self.pieces[i] = Some(body);
        self.present += 1;
        if self.present < self.pieces.len() {
            return None;
        }
        let mut whole = Vec::new();
        for p in self.pieces.iter().flatten() {
            whole.extend_from_slice(p);
        }
        self.present = 0;
        self.pieces.clear();
        Some((self.header.take()?, whole))
    }
}

/// A throwaway identity, only when the persistent one cannot be loaded.
fn ephemeral() -> Result<crate::identity::Identity> {
    let c = rcgen::generate_simple_self_signed(vec!["logistream".into()])?;
    Ok(crate::identity::Identity {
        cert: rustls::pki_types::CertificateDer::from(c.cert.der().to_vec()),
        key: rustls::pki_types::PrivateKeyDer::try_from(c.key_pair.serialize_der())
            .map_err(|e| anyhow::anyhow!("key: {e}"))?,
    })
}

pub fn server(bind: SocketAddr) -> Result<quinn::Endpoint> {
    // Spec 017, phase A: the persistent identity, and the client is asked
    // for its certificate without being required to send one.
    let id = match crate::identity::load() {
        Ok(id) => id,
        Err(e) => {
            println!("  WARNING identity unavailable ({e:#}); ephemeral certificate");
            ephemeral()?
        }
    };
    server_with(bind, id)
}

pub(crate) fn server_with(bind: SocketAddr, id: crate::identity::Identity) -> Result<quinn::Endpoint> {
    println!("  host identity: {}", crate::identity::short(&id.fingerprint()));
    let crypto = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_client_cert_verifier(Arc::new(crate::identity::ObservedClient))
        .with_single_cert(vec![id.cert], id.key)
        .context("server config")?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?,
    ));
    // Without keep-alive the NAT closes the hole in the silence between frames.
    //
    // SHORT idle timeout (5s, not 10s) because the host serves one client
    // at a time: until the dead connection expires, it accepts nobody.
    // With 10s, closing the window and clicking again right away did not
    // connect -- and the client did not even get a refusal, just silence,
    // because nobody was calling `accept`. A viewer killed by force (no
    // CONNECTION_CLOSE) is the normal case here, not the exception.
    let t = Arc::get_mut(&mut cfg.transport).expect("fresh transport");
    t.max_idle_timeout(Some(std::time::Duration::from_secs(5).try_into()?));
    t.keep_alive_interval(Some(std::time::Duration::from_secs(1)));
    quinn::Endpoint::server(cfg, bind).context("open server")
}

pub fn client(bind: SocketAddr) -> Result<quinn::Endpoint> {
    let pinned = crate::identity::pinned_from_env();
    if pinned.is_none() {
        eprintln!("  WARNING no LOGI_PAR: the host fingerprint is not checked (spec 017, phase A)");
    }
    let id = crate::identity::load()
        .map_err(|e| eprintln!("  WARNING identity unavailable ({e:#}); connecting without a certificate"))
        .ok();
    client_with(bind, id, pinned)
}

pub(crate) fn client_with(
    bind: SocketAddr,
    id: Option<crate::identity::Identity>,
    pinned: Option<String>,
) -> Result<quinn::Endpoint> {
    let mut ep = quinn::Endpoint::client(bind).context("open client")?;
    let base = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(crate::identity::PinnedHost { pinned }));
    let crypto = match id {
        Some(id) => base.with_client_auth_cert(vec![id.cert], id.key).context("client certificate")?,
        None => base.with_no_client_auth(),
    };
    let mut cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    ));

    // A datagram queue big enough for ONE whole keyframe.
    //
    // A 1440p IDR goes out as dozens of ~1.1 KB datagrams, all at the same
    // instant. Whatever does not fit the receive queue is DROPPED (datagrams
    // are not retransmitted, and that is the semantics we want for video)
    // -- and an incomplete keyframe is the worst thing to lose: the decoder
    // complains `non-existing PPS 0 referenced` and draws nothing until the
    // next IDR. 8 MB is plenty of headroom and costs memory a remote screen
    // already spends.
    let mut t = quinn::TransportConfig::default();
    t.datagram_receive_buffer_size(Some(8 * 1024 * 1024));
    cfg.transport_config(Arc::new(t));

    ep.set_default_client_config(cfg);
    Ok(ep)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves that this crate's server and client exchange datagrams.
    ///
    /// Exists because the host said "80 fps, 0 refused" and the receiver
    /// saw zero. With both ends in one process, there is one suspect at
    /// a time.
    #[tokio::test]
    async fn datagram_really_gets_through() {
        let srv = server_with("127.0.0.1:0".parse().unwrap(), temp_id("dg-host").1).expect("server");
        let addr = srv.local_addr().expect("port");

        let accepted = tokio::spawn(async move {
            let conn = srv.accept().await.expect("incoming").await.expect("handshake");
            let max = conn.max_datagram_size();
            for i in 0..5u8 {
                conn.send_datagram(Bytes::from(vec![i; 100])).expect("send");
            }
            // Without this the endpoint may close before the UDP goes out.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            max
        });

        let cli = client_with("127.0.0.1:0".parse().unwrap(), Some(temp_id("dg-cli").1), None).expect("client");
        let conn = cli
            .connect(addr, "logistream")
            .expect("connect")
            .await
            .expect("client handshake");

        let mut seen = 0;
        for _ in 0..5 {
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                conn.read_datagram(),
            )
            .await
            {
                Ok(Ok(d)) => {
                    assert_eq!(d.len(), 100);
                    seen += 1;
                }
                other => panic!("no datagram arrived: {other:?}"),
            }
        }
        let max = accepted.await.expect("task");
        assert!(max.is_some(), "the peer must accept datagrams");
        assert_eq!(seen, 5);
    }

    fn temp_id(name: &str) -> (std::path::PathBuf, crate::identity::Identity) {
        let d = std::env::temp_dir().join(format!(
            "logi-net-{name}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        let _ = std::fs::remove_dir_all(&d);
        let id = crate::identity::load_or_create(&d).expect("identity");
        (d, id)
    }

    /// Spec 017, phase A, over a real handshake: the pin decides, and the
    /// host learns who the client is.
    #[tokio::test]
    async fn pairing_by_fingerprint() {
        let (_, host) = temp_id("host");
        let (_, other) = temp_id("other");
        let (dir_cli, cli) = temp_id("cli");
        let fp_host = host.fingerprint();
        let fp_other = other.fingerprint();
        let fp_cli = cli.fingerprint();

        let srv = server_with("127.0.0.1:0".parse().unwrap(), host).expect("server");
        let addr = srv.local_addr().unwrap();
        let seen_task = tokio::spawn(async move {
            let mut seen = Vec::new();
            // the wrong pin aborts during the handshake; only the good ones arrive
            while seen.len() < 2 {
                let Some(e) = srv.accept().await else { break };
                if let Ok(c) = e.await {
                    seen.push(crate::identity::describe_peer(&c, &[fp_cli.clone()].into_iter().collect()));
                }
            }
            seen
        });

        // wrong pin: refused by the client
        let wrong = client_with("127.0.0.1:0".parse().unwrap(), None, Some(fp_other)).unwrap();
        let r = wrong.connect(addr, "logistream").unwrap().await;
        assert!(r.is_err(), "a wrong fingerprint had to be refused");

        // right pin, with identity: connects, host sees it as authorized
        let right = client_with("127.0.0.1:0".parse().unwrap(), Some(cli), Some(fp_host.clone())).unwrap();
        let _c1 = right.connect(addr, "logistream").unwrap().await.expect("the right fingerprint connects");

        // no identity (an old client): phase A still lets it in
        let old = client_with("127.0.0.1:0".parse().unwrap(), None, Some(fp_host)).unwrap();
        let _c2 = old.connect(addr, "logistream").unwrap().await.expect("an old client gets in during phase A");

        let seen = tokio::time::timeout(std::time::Duration::from_secs(5), seen_task)
            .await
            .expect("deadline")
            .unwrap();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].ends_with(" authorized") && !seen[0].contains("unknown"), "{seen:?}");
        assert_eq!(seen[1], "no certificate");
        let _ = std::fs::remove_dir_all(dir_cli);
    }

    #[test]
    fn keyframe_watch_asks_until_a_keyframe_arrives() {
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + std::time::Duration::from_millis(n);
        let mut w = KeyframeWatch::new(t0);
        assert!(!w.should_request(ms(100)), "too early");
        assert!(w.should_request(ms(300)), "nothing drawable after 300 ms");
        assert!(!w.should_request(ms(400)), "spaced out");
        assert!(w.should_request(ms(600)));
        w.frame(false);
        assert!(w.should_request(ms(900)), "a P frame does not sync");
        w.frame(true);
        assert!(!w.should_request(ms(5000)), "synced: no requests");
        w.losses(0, ms(5000));
        assert!(!w.should_request(ms(6000)), "no new loss");
        w.losses(1, ms(6000));
        assert!(!w.should_request(ms(6100)));
        assert!(w.should_request(ms(6300)), "a loss breaks the chain");
        w.frame(true);
        w.losses(1, ms(7000));
        assert!(!w.should_request(ms(8000)), "same loss count is not a new loss");
    }

    fn header() -> SliceHeader {
        SliceHeader { frame: 7, piece: 0, pieces: 1, keyframe: true, captured_at_ns: 123 }
    }

    #[test]
    fn header_round_trips() {
        let mut b = BytesMut::new();
        header().write_to(&mut b);
        assert_eq!(b.len(), HEADER_LEN, "the size counts against the MTU budget");
        let mut read = b.freeze();
        assert_eq!(SliceHeader::read_from(&mut read), Some(header()));
    }

    #[test]
    fn large_frame_is_reassembled() {
        // A 1080p keyframe is tens of KB and does NOT fit in a datagram --
        // that is why slicing exists.
        let original: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let slices = slice(&original, header(), 1200);
        assert!(slices.len() > 40, "it really has to slice: {}", slices.len());

        let mut r = Reassembler::default();
        let mut out = None;
        for f in &slices {
            if let Some(x) = r.push(f) {
                out = Some(x);
            }
        }
        let (c, data) = out.expect("reassembled");
        assert_eq!(data, original);
        assert_eq!(c.frame, 7);
        assert!(c.keyframe);
        assert_eq!(r.lost, 0);
    }

    #[test]
    fn out_of_order_also_reassembles() {
        let original: Vec<u8> = (0..9_000u32).map(|i| i as u8).collect();
        let mut slices = slice(&original, header(), 1200);
        slices.reverse(); // UDP does not promise order
        let mut r = Reassembler::default();
        let out = slices.iter().find_map(|f| r.push(f));
        assert_eq!(out.expect("reassembled").1, original);
    }

    #[test]
    fn incomplete_frame_is_abandoned_and_counted() {
        let a: Vec<u8> = vec![1; 5_000];
        let b: Vec<u8> = vec![2; 5_000];
        let mut r = Reassembler::default();
        // Half of frame 7...
        for f in slice(&a, header(), 1200).iter().take(2) {
            assert!(r.push(f).is_none());
        }
        // ...and frame 8 starts. Frame 7 becomes a loss, not stuck memory:
        // an old frame reassembled would arrive late and delay the next one.
        let c8 = SliceHeader { frame: 8, ..header() };
        let out = slice(&b, c8, 1200).iter().find_map(|f| r.push(f));
        assert_eq!(out.expect("reassembled frame 8").1, b);
        assert_eq!(r.lost, 1, "the incomplete frame 7 must be counted");
    }
}
