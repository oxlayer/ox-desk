//! Transporte: QUIC com datagramas para video, streams para controle.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Por que QUIC e nao UDP cru
//!
//! UDP+FEC e a forma certa para video -- quadro atrasado nao serve, e
//! retransmitir o que ja passou e pior que perder. Mas escrever UDP cru
//! significa escrever tambem handshake, cripto, controle de
//! congestionamento e remontagem. QUIC entrega os quatro **e e UDP**:
//! datagramas nao sao retransmitidos, que e exatamente a semantica do
//! video.
//!
//! De quebra, a identidade do par vira o certificado TLS -- pareamento
//! passa a ser "confiar nesta chave publica", que e o modelo que o
//! usuario descreveu: aceita uma vez, guarda no chaveiro.
//!
//! ## O limite de tamanho manda no formato
//!
//! Datagrama QUIC nao fragmenta: o que nao couber no MTU e recusado. Um
//! quadro-chave de 1080p tem dezenas de KB, entao ele **precisa** ser
//! fatiado por nos -- e e por isso que o cabecalho carrega
//! `frame`/`piece`/`pieces`, nao por gosto de protocolo.

use anyhow::{Context, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::net::SocketAddr;
use std::sync::Arc;

/// Cabecalho de cada fatia. 16 bytes, big-endian.
///
/// Fixo e pequeno de proposito: ele viaja em TODO datagrama, e cada byte
/// aqui sai do orcamento do MTU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceHeader {
    /// Numero do quadro. Serve para descartar fatia que chegou tarde:
    /// quadro velho nao e util, e montar ele atrasaria o que vem.
    pub frame: u32,
    pub piece: u16,
    pub pieces: u16,
    pub keyframe: bool,
    /// Nanossegundos da captura, para medir latencia fim-a-fim. Sem isso
    /// nao da para saber se uma mudanca melhorou ou piorou.
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

/// Corta um pacote codificado em fatias que cabem no datagrama.
pub fn slice(pacote: &[u8], cab: SliceHeader, max_datagram: usize) -> Vec<Bytes> {
    let util = max_datagram.saturating_sub(HEADER_LEN).max(1);
    let total = pacote.len().div_ceil(util).max(1) as u16;
    pacote
        .chunks(util)
        .enumerate()
        .map(|(i, parte)| {
            let mut b = BytesMut::with_capacity(HEADER_LEN + parte.len());
            SliceHeader { piece: i as u16, pieces: total, ..cab }.write_to(&mut b);
            b.put_slice(parte);
            b.freeze()
        })
        .collect()
}

/// Remonta um quadro a partir das fatias que chegaram.
///
/// Guarda um quadro de cada vez: assim que chega fatia de um quadro mais
/// novo, o antigo e abandonado. Segurar varios "por via das duvidas"
/// gastaria memoria para entregar quadro velho, que ninguem quer ver.
#[derive(Default)]
pub struct Reassembler {
    frame: u32,
    pieces: Vec<Option<Bytes>>,
    presentes: usize,
    cab: Option<SliceHeader>,
    /// Quadros abandonados por chegarem incompletos. E o numero que diz
    /// se vale a pena ligar FEC.
    pub perdidos: u64,
}

impl Reassembler {
    /// Devolve o quadro inteiro quando a ultima fatia dele chega.
    pub fn push(&mut self, datagrama: &[u8]) -> Option<(SliceHeader, Vec<u8>)> {
        let mut buf = datagrama;
        let cab = SliceHeader::read_from(&mut buf)?;
        let corpo = Bytes::copy_from_slice(buf);

        if cab.frame != self.frame || self.pieces.len() != cab.pieces as usize {
            if self.presentes > 0 && self.presentes < self.pieces.len() {
                self.perdidos += 1;
            }
            self.frame = cab.frame;
            self.pieces = vec![None; cab.pieces as usize];
            self.presentes = 0;
            self.cab = Some(cab);
        }
        let i = cab.piece as usize;
        if i >= self.pieces.len() || self.pieces[i].is_some() {
            return None;
        }
        self.pieces[i] = Some(corpo);
        self.presentes += 1;
        if self.presentes < self.pieces.len() {
            return None;
        }
        let mut inteiro = Vec::new();
        for p in self.pieces.iter().flatten() {
            inteiro.extend_from_slice(p);
        }
        self.presentes = 0;
        self.pieces.clear();
        Some((self.cab.take()?, inteiro))
    }
}

/// A throwaway identity, only when the persistent one cannot be loaded.
fn efemera() -> Result<crate::identity::Identity> {
    let c = rcgen::generate_simple_self_signed(vec!["logistream".into()])?;
    Ok(crate::identity::Identity {
        cert: rustls::pki_types::CertificateDer::from(c.cert.der().to_vec()),
        key: rustls::pki_types::PrivateKeyDer::try_from(c.key_pair.serialize_der())
            .map_err(|e| anyhow::anyhow!("chave: {e}"))?,
    })
}

pub fn servidor(bind: SocketAddr) -> Result<quinn::Endpoint> {
    // Spec 017, phase A: the persistent identity, and the client is asked
    // for its certificate without being required to send one.
    let id = match crate::identity::load() {
        Ok(id) => id,
        Err(e) => {
            println!("  AVISO identidade indisponivel ({e:#}); certificado efemero");
            efemera()?
        }
    };
    servidor_com(bind, id)
}

fn servidor_com(bind: SocketAddr, id: crate::identity::Identity) -> Result<quinn::Endpoint> {
    println!("  identidade do host: {}", crate::identity::short(&id.fingerprint()));
    let cripto = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_client_cert_verifier(Arc::new(crate::identity::ObservedClient))
        .with_single_cert(vec![id.cert], id.key)
        .context("config do servidor")?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(cripto)?,
    ));
    // Sem keep-alive o NAT fecha o buraco no silencio entre quadros.
    //
    // Ocioso CURTO (5s, nao 10s) porque o host serve um cliente de cada
    // vez: enquanto a conexao morta nao expira, ele nao aceita ninguem.
    // Com 10s, fechar a janela e clicar de novo em seguida nao conectava
    // -- e o cliente nem recusa recebia, so silencio, porque ninguem
    // estava chamando `accept`. Um visualizador encerrado a forca (sem
    // CONNECTION_CLOSE) e o caso normal aqui, nao a excecao.
    let t = Arc::get_mut(&mut cfg.transport).expect("transport novo");
    t.max_idle_timeout(Some(std::time::Duration::from_secs(5).try_into()?));
    t.keep_alive_interval(Some(std::time::Duration::from_secs(1)));
    quinn::Endpoint::server(cfg, bind).context("abrir servidor")
}

pub fn cliente(bind: SocketAddr) -> Result<quinn::Endpoint> {
    let fixada = crate::identity::pinned_from_env();
    if fixada.is_none() {
        eprintln!("  AVISO sem LOGI_PAR: a digital do host nao e conferida (spec 017, fase A)");
    }
    let id = crate::identity::load()
        .map_err(|e| eprintln!("  AVISO identidade indisponivel ({e:#}); conectando sem certificado"))
        .ok();
    cliente_com(bind, id, fixada)
}

fn cliente_com(
    bind: SocketAddr,
    id: Option<crate::identity::Identity>,
    fixada: Option<String>,
) -> Result<quinn::Endpoint> {
    let mut ep = quinn::Endpoint::client(bind).context("abrir cliente")?;
    let base = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(crate::identity::PinnedHost { pinned: fixada }));
    let cripto = match id {
        Some(id) => base.with_client_auth_cert(vec![id.cert], id.key).context("certificado do cliente")?,
        None => base.with_no_client_auth(),
    };
    let mut cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(cripto)?,
    ));

    // Fila de datagramas grande o bastante para UM quadro-chave inteiro.
    //
    // Um IDR de 1440p sai em dezenas de datagramas de ~1,1 KB, todos no
    // mesmo instante. O que nao couber na fila de recepcao e DESCARTADO
    // (datagrama nao retransmite, e essa e a semantica que queremos para
    // video) -- e um quadro-chave incompleto e o pior a perder: o
    // decodificador reclama `non-existing PPS 0 referenced` e nao desenha
    // nada ate o IDR seguinte. 8 MB e folga de sobra e custa memoria que
    // uma tela remota ja gasta.
    let mut t = quinn::TransportConfig::default();
    t.datagram_receive_buffer_size(Some(8 * 1024 * 1024));
    cfg.transport_config(Arc::new(t));

    ep.set_default_client_config(cfg);
    Ok(ep)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prova que servidor e cliente desta crate trocam datagrama.
    ///
    /// Existe porque o host dizia "80 fps, 0 recusados" e o receptor
    /// via zero. Com as duas pontas num processo so, sobra um suspeito
    /// de cada vez.
    #[tokio::test]
    async fn datagrama_atravessa_de_verdade() {
        let srv = servidor_com("127.0.0.1:0".parse().unwrap(), id_temp("dg-host").1).expect("servidor");
        let addr = srv.local_addr().expect("porta");

        let aceita = tokio::spawn(async move {
            let conexao = srv.accept().await.expect("entrada").await.expect("handshake");
            let max = conexao.max_datagram_size();
            for i in 0..5u8 {
                conexao.send_datagram(Bytes::from(vec![i; 100])).expect("enviar");
            }
            // Sem isto o endpoint pode fechar antes de o UDP sair.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            max
        });

        let cli = cliente_com("127.0.0.1:0".parse().unwrap(), Some(id_temp("dg-cli").1), None).expect("cliente");
        let conexao = cli
            .connect(addr, "logistream")
            .expect("connect")
            .await
            .expect("handshake cliente");

        let mut vistos = 0;
        for _ in 0..5 {
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                conexao.read_datagram(),
            )
            .await
            {
                Ok(Ok(d)) => {
                    assert_eq!(d.len(), 100);
                    vistos += 1;
                }
                outro => panic!("nao chegou datagrama: {outro:?}"),
            }
        }
        let max = aceita.await.expect("tarefa");
        assert!(max.is_some(), "o par tem de aceitar datagramas");
        assert_eq!(vistos, 5);
    }

    fn id_temp(nome: &str) -> (std::path::PathBuf, crate::identity::Identity) {
        let d = std::env::temp_dir().join(format!(
            "logi-net-{nome}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        let _ = std::fs::remove_dir_all(&d);
        let id = crate::identity::load_or_create(&d).expect("identidade");
        (d, id)
    }

    /// Spec 017, phase A, over a real handshake: the pin decides, and the
    /// host learns who the client is.
    #[tokio::test]
    async fn pareamento_pela_digital() {
        let (_, host) = id_temp("host");
        let (_, outro) = id_temp("outro");
        let (dir_cli, cli) = id_temp("cli");
        let fp_host = host.fingerprint();
        let fp_outro = outro.fingerprint();
        let fp_cli = cli.fingerprint();

        let srv = servidor_com("127.0.0.1:0".parse().unwrap(), host).expect("servidor");
        let addr = srv.local_addr().unwrap();
        let visto = tokio::spawn(async move {
            let mut vistos = Vec::new();
            // the wrong pin aborts during the handshake; only the good ones arrive
            while vistos.len() < 2 {
                let Some(e) = srv.accept().await else { break };
                if let Ok(c) = e.await {
                    vistos.push(crate::identity::describe_peer(&c, &[fp_cli.clone()].into_iter().collect()));
                }
            }
            vistos
        });

        // wrong pin: refused by the client
        let errado = cliente_com("127.0.0.1:0".parse().unwrap(), None, Some(fp_outro)).unwrap();
        let r = errado.connect(addr, "logistream").unwrap().await;
        assert!(r.is_err(), "digital errada tinha de recusar");

        // right pin, with identity: connects, host sees it as authorized
        let certo = cliente_com("127.0.0.1:0".parse().unwrap(), Some(cli), Some(fp_host.clone())).unwrap();
        let _c1 = certo.connect(addr, "logistream").unwrap().await.expect("digital certa conecta");

        // no identity (an old client): phase A still lets it in
        let velho = cliente_com("127.0.0.1:0".parse().unwrap(), None, Some(fp_host)).unwrap();
        let _c2 = velho.connect(addr, "logistream").unwrap().await.expect("cliente velho entra na fase A");

        let vistos = tokio::time::timeout(std::time::Duration::from_secs(5), visto)
            .await
            .expect("prazo")
            .unwrap();
        assert_eq!(vistos.len(), 2, "{vistos:?}");
        assert!(vistos[0].ends_with("autorizado") && !vistos[0].contains("desconhecido"), "{vistos:?}");
        assert_eq!(vistos[1], "sem certificado");
        let _ = std::fs::remove_dir_all(dir_cli);
    }

    fn cab() -> SliceHeader {
        SliceHeader { frame: 7, piece: 0, pieces: 1, keyframe: true, captured_at_ns: 123 }
    }

    #[test]
    fn cabecalho_vai_e_volta() {
        let mut b = BytesMut::new();
        cab().write_to(&mut b);
        assert_eq!(b.len(), HEADER_LEN, "o tamanho entra no orcamento do MTU");
        let mut lido = b.freeze();
        assert_eq!(SliceHeader::read_from(&mut lido), Some(cab()));
    }

    #[test]
    fn quadro_grande_e_remontado() {
        // Um quadro-chave de 1080p tem dezenas de KB e NAO cabe num
        // datagrama -- e por isso que existe fatiamento.
        let original: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let fatias = slice(&original, cab(), 1200);
        assert!(fatias.len() > 40, "tem de fatiar mesmo: {}", fatias.len());

        let mut r = Reassembler::default();
        let mut saiu = None;
        for f in &fatias {
            if let Some(x) = r.push(f) {
                saiu = Some(x);
            }
        }
        let (c, dados) = saiu.expect("remontou");
        assert_eq!(dados, original);
        assert_eq!(c.frame, 7);
        assert!(c.keyframe);
        assert_eq!(r.perdidos, 0);
    }

    #[test]
    fn fora_de_ordem_tambem_remonta() {
        let original: Vec<u8> = (0..9_000u32).map(|i| i as u8).collect();
        let mut fatias = slice(&original, cab(), 1200);
        fatias.reverse(); // UDP nao promete ordem
        let mut r = Reassembler::default();
        let saiu = fatias.iter().find_map(|f| r.push(f));
        assert_eq!(saiu.expect("remontou").1, original);
    }

    #[test]
    fn quadro_incompleto_e_abandonado_e_contado() {
        let a: Vec<u8> = vec![1; 5_000];
        let b: Vec<u8> = vec![2; 5_000];
        let mut r = Reassembler::default();
        // Metade do quadro 7...
        for f in slice(&a, cab(), 1200).iter().take(2) {
            assert!(r.push(f).is_none());
        }
        // ...e o quadro 8 comeca. O 7 vira perda, nao memoria presa:
        // quadro velho remontado chegaria tarde e atrasaria o proximo.
        let c8 = SliceHeader { frame: 8, ..cab() };
        let saiu = slice(&b, c8, 1200).iter().find_map(|f| r.push(f));
        assert_eq!(saiu.expect("remontou o 8").1, b);
        assert_eq!(r.perdidos, 1, "o quadro 7 incompleto tem de ser contado");
    }
}
