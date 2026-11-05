//! Medição e reparo do setup, no MacBook.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! Aqui vive a parte mais traiçoeira do conjunto, e por um motivo só: no
//! macOS a falta de permissão quase nunca se parece com falta de
//! permissão. O AVFoundation **trava** sem Gravação de Tela, o
//! `codesign` falha com `errSecInternalComponent` sem o chaveiro
//! destrancado, e a injeção de input é aceita e descartada sem
//! Acessibilidade. Três sintomas que não mencionam a causa.
//!
//! ## Por que as permissões se perguntam ao HOST, por fora
//!
//! O TCC amarra a concessão ao binário -- ao designated requirement
//! dele. Perguntar daqui, do processo do `logid`, daria uma resposta
//! correta sobre o programa errado. Então a medição roda
//! `logistream-host --conferir` e lê as duas linhas que ele imprime.
//!
//! ## A ordem importa, e o catálogo a impõe
//!
//! `gravacao-de-tela` e `acessibilidade` dependem de `assinatura`. Não é
//! zelo: com binário ad-hoc a amarração é pelo cdhash, que muda a cada
//! compilação, então conceder antes de assinar **desperdiça** a
//! concessão -- e só o dono da máquina pode dar de volta, à mão, na
//! interface. Isso já aconteceu duas vezes aqui.

use crate::config::Config;
use crate::setup::{Estado, Medida};
use crate::setup_comum::rodar;
use std::path::PathBuf;

const BINS: [&str; 3] = ["logid", "logistream-host", "logistream-view"];
const IDENTIDADE: &str = "logi local signing";
const CHAVEIRO: &str = "Library/Keychains/logi.keychain-db";

fn bin_dir() -> PathBuf {
    crate::sys::home_dir().join(".local/bin")
}
fn host_bin() -> PathBuf {
    bin_dir().join("logistream-host")
}
fn uid() -> u32 {
    // Mesmo caminho que `sys::macos::unit_state` usa. `id -u` daria o
    // mesmo número ao custo de um processo por medição.
    unsafe { getuid() }
}
extern "C" {
    fn getuid() -> u32;
}

pub fn medir(cfg: &Config) -> Vec<Medida> {
    // As duas permissões saem de UMA execução do host: são duas linhas
    // da mesma saída, e rodar o binário duas vezes dobraria o custo da
    // medição mais caro da lista sem responder nada a mais.
    let tcc = conferir_permissoes();
    vec![
        binarios(),
        crate::setup_comum::config_desta_maquina(cfg),
        daemon(),
        identidade(),
        assinatura(),
        tcc.0,
        tcc.1,
        launchagent(),
        crate::setup_comum::clipboard(),
        login_remoto(),
        crate::setup_comum::pares(cfg),
    ]
}

fn binarios() -> Medida {
    let dir = bin_dir();
    let faltam: Vec<&str> = BINS.iter().copied().filter(|b| !dir.join(b).exists()).collect();
    if faltam.is_empty() {
        Medida::ok("binarios", format!("{} binários em {}", BINS.len(), dir.display()))
    } else {
        Medida::falta("binarios", format!("faltam: {}", faltam.join(", ")))
    }
}

fn daemon() -> Medida {
    let rotulo = "ai.logi.logid";
    let carregado = rodar("/bin/launchctl", &["print", &format!("gui/{}/{rotulo}", uid())], 5);
    match carregado {
        Ok(_) => Medida::ok("daemon", format!("{rotulo} carregado na sessão gráfica")),
        // Processo vivo sem LaunchAgent é o caso que engana: funciona
        // agora e não volta no próximo login.
        Err(_) if crate::setup_comum::processo_vivo("logid") => {
            Medida::falta("daemon", "o processo está rodando, mas fora do launchd: não volta no próximo login")
        }
        Err(e) => Medida::falta("daemon", format!("{rotulo}: {e}")),
    }
}

fn identidade() -> Medida {
    let kc = crate::sys::home_dir().join(CHAVEIRO);
    if !kc.exists() {
        return Medida::falta("identidade", format!("sem o chaveiro {}", kc.display()));
    }
    match rodar("/usr/bin/security", &["find-identity", "-v", &kc.to_string_lossy()], 8) {
        Ok(s) if s.contains(IDENTIDADE) => Medida::ok("identidade", format!("\"{IDENTIDADE}\" no chaveiro próprio")),
        Ok(_) => Medida::falta("identidade", format!("o chaveiro existe mas não tem \"{IDENTIDADE}\"")),
        Err(e) => Medida::erro("identidade", format!("security: {e}")),
    }
}

fn assinatura() -> Medida {
    let alvo = host_bin();
    if !alvo.exists() {
        return Medida::falta("assinatura", "o host de tela não está instalado");
    }
    // `codesign -dv` escreve tudo em STDERR, inclusive em caso de
    // sucesso -- e foi isso que fez a primeira versão disto achar que
    // nenhum binário estava assinado.
    let saida = crate::sys::child("/usr/bin/codesign")
        .args(["-dv", &alvo.to_string_lossy()])
        .output();
    let Ok(o) = saida else {
        return Medida::erro("assinatura", "não consegui executar o codesign");
    };
    let texto = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    if texto.contains(IDENTIDADE) {
        Medida::ok("assinatura", "assinado com a identidade estável")
    } else if texto.contains("adhoc") || texto.contains("linker-signed") {
        Medida::falta(
            "assinatura",
            "ad-hoc / linker-signed: a permissão de tela morre no próximo deploy",
        )
    } else if texto.contains("not signed") {
        Medida::falta("assinatura", "sem assinatura nenhuma")
    } else {
        Medida::erro("assinatura", "não reconheci a saída do codesign")
    }
}

/// Lê as duas permissões do próprio host.
fn conferir_permissoes() -> (Medida, Medida) {
    let alvo = host_bin();
    if !alvo.exists() {
        let m = |id: &str| Medida::erro(id, "o host de tela não está instalado: não há a quem perguntar");
        return (m("gravacao-de-tela"), m("acessibilidade"));
    }
    // 10 s porque o binário só imprime duas linhas e sai; se passar
    // disso, algo está pendurado e pendurado não é "ok".
    let saida = rodar(&alvo.to_string_lossy(), &["--conferir"], 10);
    let ler = |texto: &str, chave: &str| -> Option<bool> {
        texto
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{chave}: ")))
            .map(|v| v.trim() == "ok")
    };
    match saida {
        Ok(texto) => {
            let uma = |id: &str, tipo: &str| match ler(&texto, id) {
                Some(true) => Medida::ok(id, format!("concedida a {}", alvo.display())),
                Some(false) => Medida::falta(id, format!("o host não tem {tipo}")),
                // Binário antigo, de antes do `--conferir`: não medir é
                // diferente de medir "falta", e dizer qual é o caso
                // poupa a tarde que eu perderia procurando no lugar
                // errado.
                None => Medida::erro(id, "o host instalado não entende --conferir: rode o deploy"),
            };
            (
                uma("gravacao-de-tela", "Gravação de Tela"),
                uma("acessibilidade", "Acessibilidade"),
            )
        }
        Err(e) => (
            Medida::erro("gravacao-de-tela", format!("--conferir: {e}")),
            Medida::erro("acessibilidade", format!("--conferir: {e}")),
        ),
    }
}

fn launchagent() -> Medida {
    let rotulo = "ai.logi.logistream-host";
    match rodar("/bin/launchctl", &["print", &format!("gui/{}/{rotulo}", uid())], 5) {
        Ok(_) => Medida::ok("launchagent", format!("{rotulo} carregado")),
        Err(e) => Medida::falta("launchagent", format!("{rotulo}: {e}")),
    }
}

fn login_remoto() -> Medida {
    // Conexão TCP em 127.0.0.1:22, não `systemsetup -getremotelogin`:
    // o `systemsetup` pede Acesso Total ao Disco em macOS recente e
    // falharia por um motivo que não tem nada a ver com a pergunta.
    // Quem responde "este Mac aceita SSH?" é o soquete.
    let (up, ms) = crate::probe::tcp_probe("127.0.0.1", 22);
    Medida::se(
        "login-remoto",
        up,
        format!("sshd responde em {:.0} ms", ms.unwrap_or(0.0)),
        "nada escutando na porta 22",
    )
}

// ------------------------------------------------------------------ reparos

pub fn arrumar(id: &str, _cfg: &Config) -> Result<String, String> {
    match id {
        "config" => crate::setup_comum::escrever_config(),
        "clipboard" => crate::setup_comum::testar_clipboard(),
        "daemon" => instalar_agente(
            "ai.logi.logid",
            &[bin_dir().join("logid").to_string_lossy().into_owned()],
            true,
        ),
        "launchagent" => instalar_agente(
            "ai.logi.logistream-host",
            &[
                host_bin().to_string_lossy().into_owned(),
                "9777".into(),
                "1".into(),
            ],
            false,
        ),
        "assinatura" => assinar(),
        "gravacao-de-tela" => pedir("gravacao-de-tela", "Privacy_ScreenCapture"),
        "acessibilidade" => pedir("acessibilidade", "Privacy_Accessibility"),
        _ => Err(format!("'{id}' não se arruma sozinho nesta máquina")),
    }
}

/// Escreve o plist e carrega o agente.
///
/// Gera o XML aqui em vez de copiar o arquivo do repo porque os plists
/// versionados têm `/Users/user` embutido -- um caminho absoluto
/// não tem como ser relativo, e `~` não expande em plist. Gerado, o
/// mesmo wizard serve qualquer conta.
fn instalar_agente(rotulo: &str, argv: &[String], run_at_load: bool) -> Result<String, String> {
    let home = crate::sys::home_dir();
    let log = home.join(".local/state").join(format!("{}.log", rotulo.rsplit('.').next().unwrap_or(rotulo)));
    std::fs::create_dir_all(home.join(".local/state")).map_err(|e| format!("criar ~/.local/state: {e}"))?;

    let args: String = argv
        .iter()
        .map(|a| format!("    <string>{}</string>\n", escapar(a)))
        .collect();
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <!-- Gerado pelo wizard de setup (logid/src/setup_mac.rs). -->\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key><string>{rotulo}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n\
         \x20 <key>RunAtLoad</key><{ral}/>\n\
         \x20 <key>KeepAlive</key><{ral}/>\n\
         \x20 <key>ProcessType</key><string>Interactive</string>\n\
         \x20 <key>StandardOutPath</key><string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key><string>{log}</string>\n\
         </dict>\n</plist>\n",
        ral = if run_at_load { "true" } else { "false" },
        log = escapar(&log.to_string_lossy()),
    );

    let dir = home.join("Library/LaunchAgents");
    std::fs::create_dir_all(&dir).map_err(|e| format!("criar {}: {e}", dir.display()))?;
    let caminho = dir.join(format!("{rotulo}.plist"));
    std::fs::write(&caminho, &plist).map_err(|e| format!("gravar {}: {e}", caminho.display()))?;

    let alvo = format!("gui/{}", uid());
    let _ = rodar("/bin/launchctl", &["bootout", &format!("{alvo}/{rotulo}")], 10);
    // O bootstrap falha com I/O error se o anterior ainda está saindo.
    // Uma pausa curta cobre, e repetir é idempotente.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let p = caminho.to_string_lossy().into_owned();
    if let Err(e) = rodar("/bin/launchctl", &["bootstrap", &alvo, &p], 15) {
        if rodar("/bin/launchctl", &["bootstrap", &alvo, &p], 15).is_err() {
            return Err(format!("escrevi o plist, mas o bootstrap falhou: {e}"));
        }
    }
    // Conferir que carregou mesmo. `bootstrap` que volta 0 e agente que
    // não aparece no `print` é um par que já vi.
    rodar("/bin/launchctl", &["print", &format!("{alvo}/{rotulo}")], 5)
        .map_err(|e| format!("carreguei e o launchd não o lista: {e}"))?;
    Ok(format!("{rotulo} instalado e carregado"))
}

fn escapar(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn assinar() -> Result<String, String> {
    let script = bin_dir().join("assinar.sh");
    let alvo = host_bin();
    if !script.exists() {
        return Err(format!("não achei {}: rode o deploy", script.display()));
    }
    // O script é deliberadamente não-fatal (ele não derruba o deploy
    // quando a identidade falta), então o exit code dele não diz se
    // assinou. Quem decide é a medição depois.
    let _ = rodar("/bin/bash", &[&script.to_string_lossy(), &alvo.to_string_lossy()], 30);
    match assinatura().estado {
        Estado::Ok => Ok("assinado com a identidade estável".into()),
        _ => Err("rodei o assinar.sh e o binário continua sem a identidade; \
                  veja ~/.local/state/assinar.log"
            .into()),
    }
}

/// Dispara o diálogo do sistema e abre o painel certo.
///
/// Os dois, e nesta ordem: o diálogo do TCC aparece **uma vez na vida do
/// binário**. Se a pessoa já respondeu um dia (ou se o host foi lançado
/// por SSH alguma vez e a chamada passou sem diálogo), a única via é a
/// interface -- e achar o item na lista é o passo em que todo mundo
/// desiste. Abrir o painel sempre custa uma janela a mais e salva a
/// tarde.
fn pedir(id: &str, painel: &str) -> Result<String, String> {
    let alvo = host_bin();
    if !alvo.exists() {
        return Err("o host de tela não está instalado".into());
    }
    let _ = rodar(&alvo.to_string_lossy(), &["--pedir"], 20);
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{painel}");
    let _ = rodar("/usr/bin/open", &[&url], 10);

    let (tela, input) = conferir_permissoes();
    let m = if id == "gravacao-de-tela" { tela } else { input };
    match m.estado {
        Estado::Ok => Ok(m.detalhe),
        // NÃO é erro: o fluxo normal é a pessoa marcar a caixa na janela
        // que acabou de abrir, e só então a resposta muda. Dizer
        // "falhou" aqui ensinaria a desconfiar de um wizard que está
        // certo.
        _ => Err(format!(
            "{} -- marque a caixa na janela que abriu e meça de novo",
            m.detalhe
        )),
    }
}
