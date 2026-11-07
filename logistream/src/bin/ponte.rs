//! Bridge between a screen host and a panel tab: QUIC on one side, a local
//! WebSocket on the other. See specs/016-sessao-e-visual.md.
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
//! Input is the wire format of `Evento`, forwarded without being interpreted.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use logistream::net::{self, Reassembler};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

/// Nobody connected in this long: the tab is gone, and so is the reason to run.
const ESPERA_ABA: Duration = Duration::from_secs(30);
/// Largest `Evento` on the wire: a tag byte and two `f64`.
const MAX_EVENTO: usize = 17;
const ORIGEM_PADRAO: &str = "http://127.0.0.1:8787";

/// Does this request come from the panel, with this session's token?
fn autorizado(caminho: &str, origem: Option<&str>, token: &str, esperada: &str) -> bool {
    !token.is_empty()
        && caminho.strip_prefix('/') == Some(token)
        && origem == Some(esperada)
}

/// Frame header for the page: bit 0 of `flags` marks a keyframe.
fn embrulhar(keyframe: bool, captured_at_ns: u64, unidade: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(9 + unidade.len());
    m.push(u8::from(keyframe));
    m.extend_from_slice(&captured_at_ns.to_be_bytes());
    m.extend_from_slice(unidade);
    m
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let alvo = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("uso: logistream-ponte <host:porta>"))?;
    let token = std::env::var("LOGI_PONTE_TOKEN").unwrap_or_default();
    anyhow::ensure!(!token.is_empty(), "falta LOGI_PONTE_TOKEN");
    let origem = std::env::var("LOGI_PONTE_ORIGEM").unwrap_or_else(|_| ORIGEM_PADRAO.into());

    let ouvinte = TcpListener::bind("127.0.0.1:0").await?;
    println!("porta {}", ouvinte.local_addr()?.port());

    let limite = tokio::time::Instant::now() + ESPERA_ABA;

    let portaria = |pedido: &Request, resposta: Response| -> Result<Response, ErrorResponse> {
        let de = pedido.headers().get("origin").and_then(|v| v.to_str().ok());
        if autorizado(pedido.uri().path(), de, &token, &origem) {
            return Ok(resposta);
        }
        eprintln!("recusei uma conexao: token ou origem errados");
        let mut negado = ErrorResponse::new(None);
        *negado.status_mut() = StatusCode::FORBIDDEN;
        Err(negado)
    };
    // Recusar nao encerra a espera: senao uma pagina qualquer derrubaria a
    // sessao so de tentar, antes de a aba chegar.
    let mut ws = loop {
        let (tcp, _) = tokio::time::timeout_at(limite, ouvinte.accept())
            .await
            .map_err(|_| anyhow::anyhow!("nenhuma aba conectou"))??;
        if let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, portaria).await {
            break ws;
        }
    };
    drop(ouvinte);

    let mut enderecos: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&alvo).await?.collect();
    enderecos.sort_by_key(|a| !a.is_ipv4());
    let destino = *enderecos.first().ok_or_else(|| anyhow::anyhow!("nao resolvi {alvo}"))?;
    let bind: std::net::SocketAddr =
        if destino.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let ep = net::cliente(bind)?;
    let conexao = ep.connect(destino, "logistream")?.await?;
    // stderr, e nao stdout: quem nos lançou lê só a porta e fecha o pipe, e um
    // `println!` num pipe fechado derruba o processo no meio da sessão.
    eprintln!("conectado em {destino}");

    let (tx_in, mut rx_in) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let conexao_in = conexao.clone();
    tokio::spawn(async move {
        let Ok(mut envio) = conexao_in.open_uni().await else { return };
        while let Some(evento) = rx_in.recv().await {
            if envio.write_all(&evento).await.is_err() {
                break;
            }
        }
    });

    let mut r = Reassembler::default();
    loop {
        tokio::select! {
            d = conexao.read_datagram() => {
                let Some((cab, unidade)) = r.push(&d?) else { continue };
                let m = embrulhar(cab.keyframe, cab.captured_at_ns, &unidade);
                ws.send(Message::Binary(m.into())).await?;
            }
            m = ws.next() => match m {
                Some(Ok(Message::Binary(b))) if b.len() <= MAX_EVENTO => {
                    let _ = tx_in.send(b.to_vec()).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Ok(()),
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    const O: &str = "http://127.0.0.1:8787";

    #[test]
    fn so_o_painel_com_o_token_entra() {
        assert!(autorizado("/abc", Some(O), "abc", O));
        assert!(!autorizado("/errado", Some(O), "abc", O));
        assert!(!autorizado("/abc", Some("https://outro.site"), "abc", O));
        assert!(!autorizado("/abc", None, "abc", O));
        assert!(!autorizado("/", Some(O), "", O));
    }

    #[test]
    fn o_quadro_leva_a_chave_e_o_instante() {
        let m = embrulhar(true, 0x0102, &[9, 8]);
        assert_eq!(m, [1, 0, 0, 0, 0, 0, 0, 1, 2, 9, 8]);
        assert_eq!(embrulhar(false, 0, &[])[0], 0);
    }
}
