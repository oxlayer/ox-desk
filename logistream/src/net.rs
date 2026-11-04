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

/// Certificado efemero, auto-assinado.
///
/// A identidade do par e a chave publica, nao uma CA: pareamento aqui e
/// "aceitei esta chave uma vez". Por enquanto e efemero -- persistir e
/// comparar e a etapa de pareamento (v5).
fn cert_proprio() -> Result<(rustls::pki_types::CertificateDer<'static>, rustls::pki_types::PrivateKeyDer<'static>)> {
    let cert = rcgen::generate_simple_self_signed(vec!["logistream".into()])?;
    let der = rustls::pki_types::CertificateDer::from(cert.cert.der().to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!("chave: {e}"))?;
    Ok((der, key))
}

/// Aceita qualquer certificado.
///
/// **Temporario e nao esconde isso.** Vale enquanto o transporte esta
/// sendo provado em LAN; a v5 troca por "confio nesta chave publica
/// especifica", que e o pareamento de verdade. Deixar isto passar
/// despercebido seria o tipo de buraco que o projeto ja pagou.
#[derive(Debug)]
struct AceitaQualquer;

impl rustls::client::danger::ServerCertVerifier for AceitaQualquer {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

pub fn servidor(bind: SocketAddr) -> Result<quinn::Endpoint> {
    let (cert, key) = cert_proprio()?;
    let mut cfg = quinn::ServerConfig::with_single_cert(vec![cert], key)
        .context("config do servidor")?;
    // Sem keep-alive o NAT fecha o buraco no silencio entre quadros.
    let t = Arc::get_mut(&mut cfg.transport).expect("transport novo");
    t.max_idle_timeout(Some(std::time::Duration::from_secs(10).try_into()?));
    t.keep_alive_interval(Some(std::time::Duration::from_secs(2)));
    quinn::Endpoint::server(cfg, bind).context("abrir servidor")
}

pub fn cliente(bind: SocketAddr) -> Result<quinn::Endpoint> {
    let mut ep = quinn::Endpoint::client(bind).context("abrir cliente")?;
    let cripto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AceitaQualquer))
        .with_no_client_auth();
    let cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(cripto)?,
    ));
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
        let srv = servidor("127.0.0.1:0".parse().unwrap()).expect("servidor");
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

        let cli = cliente("127.0.0.1:0".parse().unwrap()).expect("cliente");
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
