//! KVM por LAN: captura teclado e mouse aqui e injeta na outra maquina.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//!   logistream-kvm <host:porta> [--observar] [--dispositivo NOME]...
//!
//! Independe de qual maquina tem o dongle: o que importa e onde os
//! dispositivos estao conectados AGORA. Troca de maquina deixa de ser
//! um comando de radio para o teclado (HID++ 0x1814) e passa a ser uma
//! decisao de software -- instantanea, e reversivel sem tocar em nada.
//!
//! ## A ordem das duas operacoes perigosas
//!
//! **Conectar primeiro, agarrar depois.** Agarrado, o teclado nao
//! existe para o desktop local; se o alvo estiver fora do ar e a gente
//! ja tivesse agarrado, a pessoa ficaria sem teclado E sem destino.
//! Entao: falhou a conexao, ninguem agarra e saimos com erro.
//!
//! **Soltar antes de morrer, sempre.** Qualquer caminho de saida passa
//! por `soltar_tudo()`. O kernel tambem solta quando o fd fecha, entao
//! teclado travado para sempre nao e alcancavel -- mas as teclas que
//! estavam APERTADAS no outro lado ficariam presas, e isso e alcancavel.
//! Por isso mandamos key-up do que estiver pressionado antes de sair.
//!
//! ## Socorro
//!
//! Os dois Ctrl juntos soltam tudo e encerram. Precisa existir saida
//! que nao dependa da rede: se o alvo cair com o grab ativo, nem o
//! teclado nem o mouse chegam ao desktop local.

use std::collections::HashSet;
use std::time::Duration;

use bytes::BytesMut;
use logistream::input::Evento;
use logistream::net;

#[cfg(target_os = "linux")]
use logistream::capture_input::{e_socorro, Dispositivo};

/// Nomes padrao, se ninguem passar `--dispositivo`.
const PADRAO: [&str; 2] = ["MX Keys", "MX Anywhere"];

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("o KVM so captura no Linux por enquanto");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let observar = args.iter().any(|a| a == "--observar");
    // Alvo que aplica a propria logica de repeticao (o Mutter faz isso
    // com teclas injetadas) receberia repeticao dobrada se mandassemos
    // a do kernel junto. Quem lanca o processo sabe o SO do destino.
    let repetir = !args.iter().any(|a| a == "--sem-repeticao");
    // `--autoteste N`: conecta, NAO agarra, e empurra N movimentos pela
    // mesma ponte que um dispositivo usaria (thread -> canal -> laco
    // async -> QUIC). Existe porque o bug que isto pegou vivia
    // exatamente nessa ponte: `recv_timeout` bloqueando o executor
    // impedia o driver do QUIC de transmitir. Testar com o teclado de
    // verdade exigiria agarra-lo, e agarrar para testar e justamente o
    // que nao se quer fazer na maquina de alguem.
    let autoteste: usize = args
        .iter()
        .position(|a| a == "--autoteste")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let alvo = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:9777".into());

    let mut filtros: Vec<String> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "--dispositivo" {
            if let Some(n) = args.get(i + 1) {
                filtros.push(n.clone());
            }
        }
    }
    if filtros.is_empty() {
        filtros = PADRAO.iter().map(|s| s.to_string()).collect();
    }

    // --- escolher dispositivos -------------------------------------
    let todos = Dispositivo::listar();
    let mut escolhidos: Vec<Dispositivo> = Vec::new();
    for mut d in todos {
        if filtros.iter().any(|f| d.nome.contains(f.as_str())) {
            println!("  {} -> {}", d.caminho.display(), d.nome);
            d.definir_repeticao(repetir);
            escolhidos.push(d);
        }
    }
    // Dizer qual filtro nao achou nada.
    //
    // Dispositivo Logitech some do receptor quando dorme ou e
    // desligado, e o no de /dev/input some junto. Sem este aviso, o KVM
    // levaria so o teclado e o mouse continuaria aqui -- e o sintoma
    // seria "metade funciona", que e pior de entender que "nada
    // funciona". Aconteceu num teste: o mouse estava dormindo e so o
    // teclado entrou, em silencio.
    for f in &filtros {
        if !escolhidos.iter().any(|d| d.nome.contains(f.as_str())) {
            eprintln!("  AVISO: nada casou com '{f}' -- dispositivo dormindo ou desligado?");
        }
    }

    if escolhidos.is_empty() {
        anyhow::bail!(
            "nenhum dispositivo casou com {filtros:?}. Sem permissao? \
             Veja .claude/skills/dar-acesso-a-input.md -- sem a regra de \
             udev os nos existem mas nao abrem, e a lista sai vazia \
             exatamente como se o dispositivo nao estivesse conectado."
        );
    }

    // --- conectar ANTES de agarrar ---------------------------------
    let conexao = if observar {
        println!("  modo observar: nao agarra e nao manda nada");
        None
    } else {
        let mut enderecos: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host(&alvo).await?.collect();
        enderecos.sort_by_key(|a| !a.is_ipv4());
        let destino =
            *enderecos.first().ok_or_else(|| anyhow::anyhow!("nao resolvi {alvo}"))?;
        let bind: std::net::SocketAddr =
            if destino.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
        let ep = net::cliente(bind)?;
        // Falhar aqui e o caso BOM: ninguem agarrou nada ainda.
        let c = ep
            .connect(destino, "logistream")?
            .await
            .map_err(|e| anyhow::anyhow!("nao conectei em {alvo}: {e}. Nada foi agarrado."))?;
        println!("  conectado em {alvo}");
        Some(c)
    };

    let envio = match &conexao {
        Some(c) => Some(c.open_uni().await?),
        None => None,
    };

    // --- agarrar ----------------------------------------------------
    if !observar && autoteste == 0 {
        for d in &mut escolhidos {
            d.agarrar().map_err(|e| {
                anyhow::anyhow!("nao agarrei {}: {e}", d.nome)
            })?;
        }
        println!("  agarrado; os dois Ctrl juntos devolvem o controle");
    }

    // --- ler em threads, uma por dispositivo ------------------------
    //
    // Uma thread por dispositivo porque `read` em evdev bloqueia, e o
    // teclado nao pode esperar o mouse se mexer.
    // Canal do TOKIO, nao do std.
    //
    // A primeira versao usava `std::sync::mpsc` e `recv_timeout` no
    // laco principal. `recv_timeout` BLOQUEIA a thread -- e num runtime
    // `current_thread` a thread e o executor inteiro, incluindo o
    // driver do QUIC, que e quem efetivamente poe os pacotes na rede.
    //
    // O resultado foi o pior tipo de bug: parecia funcionar. A captura
    // lia, o `write_all` enfileirava, e de vez em quando um punhado de
    // eventos escapava na janela em que o driver conseguia rodar. O
    // host do outro lado registrava "input conectado" e logo "input
    // desconectado", sem um unico evento aplicado no meio.
    //
    // As threads de dispositivo continuam sendo threads de verdade
    // (`read` em evdev bloqueia mesmo) e falam com o laco por
    // `blocking_send`, que e a ponte correta entre as duas metades.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Evento>(1024);

    if autoteste > 0 {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for _ in 0..autoteste {
                if tx.blocking_send(Evento::MouseRel { dx: 10.0, dy: 0.0 }).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        });
        println!("  autoteste: {autoteste} movimentos de +10 em X, sem agarrar");
    }

    for mut d in escolhidos {
        if autoteste > 0 {
            break; // nao ler dispositivo de verdade no autoteste
        }
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            match d.ler() {
                Ok(eventos) => {
                    for e in eventos {
                        if tx.blocking_send(e).is_err() {
                            return; // o main saiu; soltar vem do Drop
                        }
                    }
                }
                Err(err) => {
                    eprintln!("  {} parou: {err}", d.nome);
                    return;
                }
            }
        });
    }
    drop(tx);

    // --- repassar ---------------------------------------------------
    let mut buf = BytesMut::new();
    let mut pressionadas: HashSet<u32> = HashSet::new();
    let mut envio = envio;
    let motivo;

    loop {
        let e = match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(e)) => e,
            // Todas as threads de dispositivo morreram.
            Ok(None) => {
                motivo = "os dispositivos pararam de responder".into();
                break;
            }
            Err(_) => {
                if let Some(c) = &conexao {
                    if let Some(r) = c.close_reason() {
                        motivo = format!("conexao caiu: {r}");
                        break;
                    }
                }
                continue;
            }
        };

        // Contabilidade de teclas presas. Tem de valer TAMBEM em
        // `--observar`, senao o primeiro uso real comeca com o conjunto
        // mentindo.
        if let Evento::Tecla { keycode, apertado } = e {
            if apertado {
                pressionadas.insert(keycode);
            } else {
                pressionadas.remove(&keycode);
            }
        }

        if e_socorro(&pressionadas) {
            motivo = "socorro: os dois Ctrl".into();
            break;
        }

        if observar {
            println!("  {e:?}");
            continue;
        }

        buf.clear();
        e.escrever(&mut buf);
        if let Some(s) = &mut envio {
            if let Err(err) = s.write_all(&buf).await {
                motivo = format!("escrita falhou: {err}");
                break;
            }
        }
    }

    // --- soltar o que ficou apertado do outro lado ------------------
    //
    // Sem isto, encerrar com Shift apertado deixa a outra maquina
    // digitando em maiuscula para sempre. Ja aconteceu no caminho do
    // visualizador (Mac -> Ubuntu) e custou um reinicio de sessao.
    if let Some(s) = &mut envio {
        for k in &pressionadas {
            buf.clear();
            Evento::Tecla { keycode: *k, apertado: false }.escrever(&mut buf);
            let _ = s.write_all(&buf).await;
        }
        let _ = s.finish();
    }
    // Uma volta para os ultimos bytes saírem antes de fechar o socket.
    tokio::time::sleep(Duration::from_millis(120)).await;

    println!("  {motivo}; {} tecla(s) soltas; controle de volta", pressionadas.len());
    Ok(())
}
