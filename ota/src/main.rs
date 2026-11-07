//! `logi-ota` -- a maquina se atualiza sozinha pelas Releases do GitHub.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Binario separado, e nao uma thread do `logid`, por tres razoes que
//! aparecem juntas:
//!
//! 1. **Quem troca o binario nao pode morrer com ele.** O reinicio do
//!    daemon mata tudo no cgroup do servico no Linux e no grupo de
//!    processos do job no launchd. Uma thread dentro do `logid` nao
//!    sobreviveria ao proprio reinicio que ela pediu -- e nao haveria
//!    quem conferisse o `/healthz` nem quem revertesse.
//! 2. **O OTA precisa funcionar com o daemon morto.** E justamente o
//!    caso em que atualizar resolve. Se o relogio vivesse no `logid`,
//!    um `logid` quebrado nunca mais se consertaria.
//! 3. O relogio pertence ao supervisor do sistema (timer do systemd,
//!    `StartInterval` do launchd, tarefa agendada), que ja existe nas
//!    tres maquinas e ja e quem cuida de tudo que precisa acordar.
//!
//!   uso:  logi-ota [--conferir | --forcar | --estado]
//!
//! Sem argumento: confere e aplica se houver novidade.

mod aplicar;
mod estado;
mod exec;
mod lancamento;
mod plano;
mod sha256;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let modo = match args.first().map(String::as_str) {
        None => aplicar::Modo::Aplicar,
        Some("--conferir") => aplicar::Modo::Conferir,
        Some("--forcar") => aplicar::Modo::Forcar,
        Some("--estado") => return mostrar_estado(args.iter().any(|a| a == "--curto")),
        Some("-h") | Some("--help") => {
            println!(
                "logi-ota [--conferir | --forcar | --estado]

  (sem argumento)  confere a Release mais recente e aplica se for mais nova
  --conferir       so confere e grava o estado; nao troca nada
  --forcar         aplica mesmo que o carimbo seja o mesmo
  --estado         imprime o JSON da ultima checagem
  --estado --curto uma linha: <resultado> TAB <idade em s> TAB <frase>

Token de leitura das Releases (repo privado) em ~/.config/logi/ota-token
ou na variavel LOGI_OTA_TOKEN. Ver specs/015-ota.md."
            );
            return ExitCode::SUCCESS;
        }
        Some(outro) => {
            eprintln!("logi-ota: nao conheco {outro}; tente --help");
            return ExitCode::from(2);
        }
    };

    match aplicar::rodar(modo) {
        Ok(e) => {
            println!("{}: {}", e.resultado, e.linha());
            ExitCode::SUCCESS
        }
        Err(e) => {
            // stderr: isto roda por tarefa agendada, e o que importa e
            // ficar no log do supervisor.
            eprintln!("{}: {}", e.resultado, if e.resultado == "erro" { &e.mensagem } else { &e.mensagem });
            ExitCode::FAILURE
        }
    }
}

/// A ultima checagem, para quem consome.
///
/// `--curto` existe para o passo `ota` do wizard: o `logid` nao linka
/// esta crate (sao binarios separados) e **nao deve reimplementar nem o
/// formato do arquivo nem as frases**. Entao quem pergunta roda
/// `logi-ota --estado --curto` e corta em TAB. Um dono do formato, um
/// contrato de uma linha.
fn mostrar_estado(curto: bool) -> ExitCode {
    let ctx = match aplicar::contexto(aplicar::Modo::Conferir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("logi-ota: {e}");
            return ExitCode::FAILURE;
        }
    };
    match estado::ler(&ctx.dir) {
        Some(e) => {
            if curto {
                println!("{}", estado::curto(&e, estado::agora()));
            } else {
                println!("{}", serde_json::to_string_pretty(&e).unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("ainda nao conferi nada (nada em {})", estado::caminho(&ctx.dir).display());
            ExitCode::FAILURE
        }
    }
}
