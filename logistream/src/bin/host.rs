//! Host de tela: despacha para a implementacao desta plataforma.
//!
//! Spec: ../../specs/010-remote-desktop.md
//!
//! Um binario com o MESMO nome nas duas maquinas de proposito: quem
//! acorda o host (o painel, por SSH) nao precisa saber se do outro lado
//! ha PipeWire ou DXGI. O que muda e o segundo argumento -- conector de
//! video no Linux, indice de monitor no Windows -- porque a unidade de
//! "uma tela" e diferente nos dois, e inventar um nome comum seria
//! esconder isso.
//!
//!   logistream-host [porta] [conector|monitor]
//!
//! ## Sem console no Windows
//!
//! Este binario sobe por tarefa agendada com `/IT`, na sessao
//! interativa -- e uma tarefa que roda um programa de console abre uma
//! **janela de console visivel**, bem no meio da tela que voce esta
//! assistindo. O usuario viu isso como "esse terminal que abre ao
//! conectar a tela ou trocar o KVM".
//!
//! O `logid` ja resolvia com `windows_subsystem = "windows"`; aqui
//! faltava. Mas desligar o console tiraria junto o log, que e a
//! ferramenta que mais usamos para diagnosticar este processo -- entao
//! o programa redireciona a propria saida para arquivo antes de
//! escrever a primeira linha.

// Sem console. O log sai por `redirecionar_saida()`, abaixo.
#![cfg_attr(windows, windows_subsystem = "windows")]

/// Manda stdout e stderr para o arquivo de log.
///
/// Tem de rodar **antes de qualquer `println!`**: o Rust resolve o
/// handle de stdout na primeira escrita e o guarda, entao trocar
/// depois nao teria efeito sobre o que ja foi resolvido.
///
/// O arquivo e deliberadamente vazado (`into_raw_handle` sem fechar):
/// ele precisa viver enquanto o processo viver, e amarra-lo a uma
/// variavel so criaria a chance de alguem solta-lo.
#[cfg(windows)]
fn redirecionar_saida() {
    use std::os::windows::io::{AsRawHandle, IntoRawHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};

    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let dir = std::path::PathBuf::from(base).join("logid");
    let _ = std::fs::create_dir_all(&dir);
    let Ok(f) = std::fs::File::create(dir.join("logistream-host.log")) else {
        return;
    };
    let h = HANDLE(f.as_raw_handle() as _);
    unsafe {
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
        let _ = SetStdHandle(STD_ERROR_HANDLE, h);
    }
    let _ = f.into_raw_handle();
}

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    redirecionar_saida();

    let a: Vec<String> = std::env::args().skip(1).collect();

    // `--conferir` e `--pedir` existem NESTE binario e nao no daemon
    // porque o TCC do macOS amarra a concessao ao binario -- ao
    // designated requirement dele. O `logid` perguntando "tenho
    // permissao de tela?" receberia uma resposta correta sobre o
    // programa errado, e o wizard de setup pintaria verde uma permissao
    // que o host nao tem.
    //
    // A saida e de UMA linha por permissao, `chave: ok|falta`, porque
    // quem le e codigo. Ver logid/src/setup_mac.rs.
    #[cfg(target_os = "macos")]
    if a.first().is_some_and(|v| v == "--conferir" || v == "--pedir") {
        let pedir = a[0] == "--pedir";
        if pedir {
            // Mostra o dialogo do sistema. So funciona lancado da
            // sessao grafica: por SSH nao ha dialogo para mostrar e a
            // chamada volta `false` sem ninguem ter visto nada.
            let _ = logistream::input_mac::pedir_permissoes();
        }
        // Medir DEPOIS de pedir, e sempre pelo preflight: o retorno do
        // pedido e o que o usuario acabou de clicar, e no caso comum
        // (dialogo ja respondido um dia) nem dialogo ha. Quem responde
        // a pergunta "da para capturar agora?" e o preflight.
        println!(
            "gravacao-de-tela: {}",
            if logistream::input_mac::tem_gravacao_de_tela() { "ok" } else { "falta" }
        );
        println!(
            "acessibilidade: {}",
            if logistream::input_mac::tem_acessibilidade() { "ok" } else { "falta" }
        );
        return Ok(());
    }

    // `--bind` decide em QUAL endereco escutar, e o padrao continua
    // sendo 0.0.0.0 -- nenhuma maquina muda de comportamento sem
    // alguem pedir.
    //
    // `--bind overlay` existe para a maquina que SAI DE CASA. O canal
    // de tela cifra e nao autentica (ver net.rs), entao em 0.0.0.0 num
    // Wi-Fi de hotel ele serve a tela e o teclado para a rede inteira.
    // Ver specs/012.
    let bind = match a.iter().position(|v| v == "--bind") {
        Some(i) => match a.get(i + 1).map(|v| logistream::bind::Modo::parse(v)) {
            Some(Ok(m)) => m,
            Some(Err(e)) => anyhow::bail!("{e}"),
            None => anyhow::bail!("--bind precisa de um valor: todas | overlay | <endereco>"),
        },
        None => logistream::bind::Modo::Todas,
    };
    match logistream::bind::resolver(&bind, &logistream::bind::locais()) {
        Ok(ip) => logistream::bind::definir(ip),
        // Falhar aqui e o desenho, nao um acidente: quem pediu overlay
        // prefere host que nao sobe a host aberto para a rede errada.
        Err(e) => anyhow::bail!("{e}"),
    }

    // Teto de banda. Existe para a sessao que sai de casa: o gargalo
    // deixa de ser a LAN e passa a ser o UPLOAD da casa, e 15 Mbps por
    // um upload de 10 nao da imagem ruim -- da fila no roteador,
    // latencia subindo e tela desmontando. Ver specs/012.
    match logistream::qualidade::teto_de_args(&a) {
        Ok(Some(bits)) => logistream::qualidade::limitar(bits),
        Ok(None) => {}
        Err(e) => anyhow::bail!("{e}"),
    }

    // Posicionais ficam depois do parse das flags, e filtrados: sem
    // isto `--bind` viraria "porta" e o host subiria na 9777 por
    // acidente de parsing.
    let pos: Vec<&String> = {
        let mut v = Vec::new();
        let mut pular = false;
        for x in &a {
            if pular { pular = false; continue; }
            if x == "--bind" || x == "--mbps" { pular = true; continue; }
            if x.starts_with("--") { continue; }
            v.push(x);
        }
        v
    };
    let porta: u16 = pos.first().and_then(|v| v.parse().ok()).unwrap_or(9777);

    #[cfg(target_os = "linux")]
    {
        let conector = pos.get(1).map(|s| s.to_string()).unwrap_or_else(|| "DP-1".into());
        logistream::host_linux::executar(porta, &conector)
    }

    #[cfg(windows)]
    {
        let monitor: u32 = pos.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        logistream::host_win::executar(porta, monitor)
    }

    #[cfg(target_os = "macos")]
    {
        // Indice da TELA no avfoundation, nao conector de video: a
        // numeracao inclui cameras, entao a tela raramente e "0".
        let indice = pos.get(1).map(|s| s.to_string()).unwrap_or_else(|| "1".into());
        logistream::host_mac::executar(porta, &indice)
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (porta, pos);
        anyhow::bail!(
            "este sistema ainda nao sabe servir a propria tela; aqui ele e o \
             cliente -- use `logistream-view`"
        )
    }
}
