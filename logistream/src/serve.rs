//! O laco de video: quadro codificado -> datagramas QUIC.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! Esta parte e **igual** no Linux e no Windows: fatiar, mandar, contar,
//! e encerrar quando ninguem esta mais assistindo. Ficou aqui quando o
//! Windows virou host, e nao copiada para o segundo binario, porque foi
//! justamente aqui que moraram os bugs caros -- o `send_datagram` que
//! falhava calado, a contagem de fps que mentia, a saida que travava no
//! meio da destruicao do NVENC. Duas copias disso divergem, e a copia
//! errada e sempre a que ninguem esta olhando.
//!
//! O que NAO esta aqui e o que de fato difere: de onde vem o quadro
//! (PipeWire x DXGI) e como o input e aplicado (D-Bus x SendInput).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::net::{self, SliceHeader};

/// Quanto esperar por outro cliente antes de encerrar o host.
pub const OCIOSO_S: u64 = 20;

/// Quanto esperar pelo PRIMEIRO cliente antes de desistir.
///
/// O host sobe sob demanda -- o painel o acorda e abre o visualizador em
/// seguida -- entao se ninguem conectou em meio minuto, nao vai mais. Sem
/// este limite, uma tentativa que falhou do lado do cliente deixava a
/// captura e o NVENC rodando na maquina de outra pessoa indefinidamente,
/// porque `accept()` sozinho espera para sempre.
pub const ESPERA_PRIMEIRO_S: u64 = 30;

/// Nanossegundos do relogio de parede.
///
/// De parede e nao monotonico de proposito: o carimbo viaja para a outra
/// maquina, e so da para medir latencia fim-a-fim se os dois lados
/// falarem da mesma origem. Com NTP na LAN o erro e de milissegundos.
pub fn agora_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// As tres bandeiras entre a captura e a rede.
///
/// Juntas numa struct e nao tres `Arc<AtomicBool>` soltos: elas sempre
/// viajam juntas, e tres parametros booleanos seguidos numa assinatura e
/// um convite a passar na ordem errada.
///
/// Bandeira e nao canal porque toda mensagem aqui e idempotente: dois
/// pedidos de quadro-chave valem um, e "saia" dito duas vezes e "saia".
#[derive(Default)]
pub struct Sinais {
    /// A rede decidiu encerrar; quem para de verdade e a captura, que e
    /// a dona do NVENC. Matar do outro lado travava no meio da
    /// destruicao dos recursos de CUDA.
    sair: AtomicBool,
    /// O proximo quadro tem de ser IDR.
    idr: AtomicBool,
    /// Ha alguem assistindo.
    cliente: AtomicBool,
}

impl Sinais {
    pub fn deve_sair(&self) -> bool {
        self.sair.load(Ordering::Relaxed)
    }

    /// Consome o pedido de quadro-chave. Devolve `true` uma vez por
    /// pedido -- quem chama repassa ao codificador.
    pub fn pegar_idr(&self) -> bool {
        self.idr.swap(false, Ordering::Relaxed)
    }

    /// Vale a pena codificar agora?
    ///
    /// Sem ninguem assistindo, codificar e queimar GPU na maquina de
    /// outra pessoa: o quadro iria para um canal que ninguem drena e
    /// seria descartado. Medido no Windows -- 120 quadros descartados
    /// antes de o primeiro cliente conectar.
    pub fn tem_cliente(&self) -> bool {
        self.cliente.load(Ordering::Relaxed)
    }
}

/// Um quadro ja codificado, esperando a rede.
pub struct Quadro {
    pub dados: Vec<u8>,
    pub keyframe: bool,
    pub captured_at_ns: u64,
}

/// Aceita um cliente, transmite enquanto ele estiver lá, e pede parada.
///
/// `abrir_input` recebe a conexao e devolve a tarefa que aplica os
/// eventos. E closure, e nao trait, porque o lado do Linux precisa
/// capturar uma conexao D-Bus que **nao e Send** (roda em `LocalSet`,
/// com `spawn_local`) enquanto o do Windows usa `tokio::spawn` normal --
/// e as duas devolvem o mesmo `JoinHandle`.
pub async fn atender(
    ep: &quinn::Endpoint,
    rx: &mut tokio::sync::mpsc::Receiver<Quadro>,
    sinais: &Sinais,
    mut abrir_input: impl FnMut(&quinn::Connection) -> tokio::task::JoinHandle<()>,
) -> anyhow::Result<()> {
    loop {
        let entrando = match tokio::time::timeout(
            Duration::from_secs(ESPERA_PRIMEIRO_S),
            ep.accept(),
        )
        .await
        {
            Ok(Some(e)) => e,
            // Endpoint fechado: nao ha mais o que aceitar.
            Ok(None) => break,
            Err(_) => {
                println!("  ninguem conectou em {ESPERA_PRIMEIRO_S}s; encerrando");
                sinais.sair.store(true, Ordering::Relaxed);
                return Ok(());
            }
        };
        let conexao = entrando.await?;
        println!("  conectado: {}", conexao.remote_address());
        // O cliente novo nao tem contexto: o proximo quadro TEM de ser
        // quadro-chave, senao a janela dele fica preta ate o IDR
        // seguinte do GOP -- ate 2s olhando nada.
        sinais.idr.store(true, Ordering::Relaxed);
        sinais.cliente.store(true, Ordering::Relaxed);
        let Some(max) = conexao.max_datagram_size() else {
            eprintln!("  ERRO: o par nao aceita datagramas QUIC; nada de video vai passar");
            continue;
        };
        println!("  datagrama maximo: {max} bytes");

        // Input por stream unidirecional: perder um "tecla solta"
        // deixaria a tecla presa no host, entao nao pode ser datagrama.
        let entrada = abrir_input(&conexao);

        let mut quadro: u32 = 0;
        let mut enviados = 0u64;
        let mut bytes = 0u64;
        let mut recusados = 0u64;
        let t0 = Instant::now();

        loop {
            let c = match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(_) => {
                    if conexao.close_reason().is_some() {
                        break;
                    }
                    continue;
                }
            };

            let cab = SliceHeader {
                frame: quadro,
                piece: 0,
                pieces: 0,
                keyframe: c.keyframe,
                captured_at_ns: c.captured_at_ns,
            };
            for fatia in net::slice(&c.dados, cab, max) {
                bytes += fatia.len() as u64;
                if let Err(err) = conexao.send_datagram(fatia) {
                    if !matches!(err, quinn::SendDatagramError::TooLarge) {
                        eprintln!("  datagrama recusado: {err}");
                    }
                    recusados += 1;
                    break;
                }
            }
            quadro = quadro.wrapping_add(1);
            enviados += 1;
            if enviados % 120 == 0 {
                let s = t0.elapsed().as_secs_f64();
                println!(
                    "  {enviados} quadros, {:.1} fps, {:.1} Mbps, {recusados} recusado(s)",
                    enviados as f64 / s,
                    bytes as f64 * 8.0 / s / 1_000_000.0
                );
            }
            if conexao.close_reason().is_some() {
                break;
            }
        }
        entrada.abort();
        sinais.cliente.store(false, Ordering::Relaxed);
        println!("  cliente saiu");

        // Sem ninguem assistindo, capturar e codificar so queima GPU na
        // maquina de outra pessoa. O host sobe sob demanda (o daemon o
        // acorda quando alguem clica), entao sair e barato -- e e o unico
        // jeito de ele nao virar processo esquecido rodando a 80 fps a
        // noite inteira.
        //
        // A janela de espera existe para reconexao: cair o wifi por dois
        // segundos nao deveria exigir voltar ao painel.
        println!("  aguardando {OCIOSO_S}s por outro cliente antes de sair");
        if tokio::time::timeout(Duration::from_secs(OCIOSO_S), ep.accept()).await.is_ok() {
            // Reconectou dentro da janela. Nao da para devolver a conexao
            // ao laco de fora sem duplicar a logica, entao saimos e o
            // daemon religa no proximo clique -- custa um clique, nao uma
            // sessao travada.
            println!("  alguem voltou, mas a conexao nao volta para o laco; encerrando");
        } else {
            println!("  ninguem voltou; encerrando");
        }
        // Pede para a captura parar; ela encerra o processo na ordem
        // certa. Matar daqui travava no meio da destruicao do NVENC.
        sinais.sair.store(true, Ordering::Relaxed);
        return Ok(());
    }
    Ok(())
}
