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

const BINS: [&str; 4] = ["logid", "logistream-host", "logistream-view", "logi-ota"];
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
        ffmpeg(),
        identidade(),
        assinatura(),
        tcc.0,
        tcc.1,
        launchagent(),
        crate::setup_comum::clipboard(),
        login_remoto(),
        tailscale(),
        crate::setup_comum::pares(cfg),
        ota(),
    ]
}

fn token_ota() -> PathBuf {
    crate::sys::home_dir().join(".config/logi/ota-token")
}

fn ota() -> Medida {
    crate::setup_comum::ota(&bin_dir().join("logi-ota"), &token_ota(), relogio())
}

/// O LaunchAgent que acorda o `logi-ota`.
///
/// `launchctl print` e não `list`: o `list` responde pelo rótulo mesmo
/// quando o job está carregado de um plist que não existe mais, e aqui
/// a pergunta é "o launchd vai acordar isto?".
fn relogio() -> Result<String, String> {
    let rotulo = "ai.logi.ota";
    let saida = rodar("/bin/launchctl", &["print", &format!("gui/{}/{rotulo}", uid())], 5)
        .map_err(|e| format!("{rotulo}: {e}"))?;
    // O intervalo sai do próprio launchd, não do que escrevemos: plist
    // editado à mão (ou de uma versão anterior) apareceria como o
    // intervalo de hoje se a frase viesse de uma constante nossa.
    let s = saida
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("interval ="))
        .and_then(|l| l.split('=').nth(1))
        .map(|v| v.trim().to_string());
    Ok(match s {
        Some(seg) => format!("LaunchAgent a cada {seg} s"),
        None => format!("{rotulo} carregado (o launchd não informou intervalo)"),
    })
}

fn binarios() -> Medida {
    let dir = bin_dir();
    let faltam: Vec<&str> = BINS.iter().copied().filter(|b| !dir.join(b).exists()).collect();
    if !faltam.is_empty() {
        return Medida::falta("binarios", format!("faltam: {}", faltam.join(", ")));
    }
    // Existir no disco não é estar no ar. Ver
    // `setup_comum::veredito_do_carimbo`: um deploy pode instalar o
    // arquivo novo e ninguém executá-lo.
    if let Some(p) = crate::setup_comum::daemon_do_disco(&dir.join("logid")) {
        return Medida::falta("binarios", p);
    }
    Medida::ok("binarios", format!("{} binários em {}", BINS.len(), dir.display()))
}

fn daemon() -> Medida {
    let rotulo = "ai.logi.logid";
    let carregado = rodar("/bin/launchctl", &["print", &format!("gui/{}/{rotulo}", uid())], 5);
    match carregado {
        // Carregado não é rodando. O `print` traz `state` e
        // `last exit code`; ignorá-los aprovava um job em crash-loop --
        // ver `setup_comum::veredito_do_launchd`.
        Ok(saida) => match crate::setup_comum::veredito_do_launchd(&saida) {
            Some(p) => Medida::falta("daemon", format!("{rotulo}: {p}")),
            None => Medida::ok("daemon", format!("{rotulo} rodando na sessão gráfica")),
        },
        // Processo vivo sem LaunchAgent é o caso que engana: funciona
        // agora e não volta no próximo login.
        Err(_) if crate::setup_comum::processo_vivo("logid") => {
            Medida::falta("daemon", "o processo está rodando, mas fora do launchd: não volta no próximo login")
        }
        Err(e) => Medida::falta("daemon", format!("{rotulo}: {e}")),
    }
}

/// As dylibs do brew que os binarios instalados pedem existem aqui?
///
/// O soname do ffmpeg entra no binario. Um `logistream-host` vindo do
/// pacote da CI pede `libavcodec.61` sob o prefixo `ffmpeg@7`; se a
/// formula nao estiver instalada, o `dyld` recusa o processo e o
/// AVFoundation nunca e alcancado. Perguntar ao `otool` o que o arquivo
/// pede, e conferir se cada caminho existe, e barato e responde
/// exatamente isso.
///
/// Ver FFMPEG_FORMULA em scripts/construir-binarios.sh: as duas pontas
/// declaram o mesmo prefixo, e o build quebra se divergirem.
fn ffmpeg() -> Medida {
    let host = bin_dir().join("logistream-host");
    if !host.exists() {
        return Medida::falta("ffmpeg", "sem o logistream-host instalado para perguntar");
    }
    let saida = match rodar("/usr/bin/otool", &["-L", &host.to_string_lossy()], 10) {
        Ok(s) => s,
        Err(e) => return Medida::erro("ffmpeg", format!("otool: {e}")),
    };
    let pedidas: Vec<&str> = saida
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("/opt/"))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    let faltam: Vec<&&str> =
        pedidas.iter().filter(|c| !std::path::Path::new(c).exists()).collect();
    if !faltam.is_empty() {
        return Medida::falta(
            "ffmpeg",
            format!(
                "{} dylib(s) que o host pede não existem aqui, a começar por {}",
                faltam.len(),
                faltam[0]
            ),
        );
    }
    match pedidas.iter().find(|c| c.contains("libavcodec")) {
        Some(c) => Medida::ok("ffmpeg", format!("{} dylib(s) do brew resolvem; {c}", pedidas.len())),
        // Sem libavcodec o binario nem e o host de tela -- ou e um build
        // de outra plataforma. Dizer "ok" aqui seria aprovar o que nao
        // foi medido.
        None => Medida::falta("ffmpeg", "o logistream-host instalado não pede libavcodec"),
    }
}

fn identidade() -> Medida {
    let kc = crate::sys::home_dir().join(CHAVEIRO);
    if !kc.exists() {
        return Medida::falta("identidade", format!("sem o chaveiro {}", kc.display()));
    }
    // Destrancar antes de perguntar. Por SSH o chaveiro esta trancado e
    // `security` responde "User interaction is not allowed" -- que nao
    // e "nao existe". A senha nao e segredo de proposito (ver
    // mac/criar-identidade.sh): o chaveiro guarda UMA chave
    // auto-assinada cujo unico poder e assinar os nossos binarios.
    let _ = rodar("/usr/bin/security", &["unlock-keychain", "-p", "logi", &kc.to_string_lossy()], 8);

    // `find-identity` SEM `-v`.
    //
    // Com `-v` o `security` lista so identidades **validas**, e valido
    // ali significa confiada pelo sistema. Um certificado auto-assinado
    // nunca e: ele sai como `CSSMERR_TP_NOT_TRUSTED` e o `-v` devolve
    // "0 valid identities found". Medido no MacBook, com a identidade
    // presente e o binario assinado por ela no mesmo minuto.
    //
    // Isso importa porque o diagnostico errado mandava recriar a
    // identidade -- e recriar gera chave nova, muda o designated
    // requirement, e o TCC DESCARTA a concessao de Gravacao de Tela.
    // O conserto teria causado exatamente o estrago que o
    // `criar-identidade.sh` e idempotente para evitar.
    match rodar("/usr/bin/security", &["find-identity", &kc.to_string_lossy()], 8) {
        Ok(s) if s.contains(IDENTIDADE) => {
            Medida::ok("identidade", format!("\"{IDENTIDADE}\" no chaveiro próprio"))
        }
        Ok(_) => Medida::falta("identidade", format!("o chaveiro existe mas não tem \"{IDENTIDADE}\"")),
        Err(e) => Medida::erro("identidade", format!("security: {e}")),
    }
}

fn assinatura() -> Medida {
    let alvo = host_bin();
    if !alvo.exists() {
        return Medida::falta("assinatura", "o host de tela não está instalado");
    }
    // `codesign -dvvv`, nao `-dv`.
    //
    // Dois achados na mesma linha. (1) O `codesign` escreve tudo em
    // STDERR, inclusive no sucesso -- a primeira versao disto lia so o
    // stdout e achava que nada estava assinado. (2) `-dv` **nao
    // imprime a linha `Authority`**; ela so aparece a partir de
    // `-dvv`. Com `-dv` a saida nao tem nem "Authority", nem "adhoc",
    // nem "not signed", e o passo caia no ramo "nao reconheci" --
    // sobre um binario corretamente assinado.
    let saida = crate::sys::child("/usr/bin/codesign")
        .args(["-dvvv", &alvo.to_string_lossy()])
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
/// Roda o `--conferir` do host como job próprio do launchd.
///
/// Como filho do `logid`, a resposta seria sobre o `logid`: o macOS
/// atribui a permissão ao processo responsável. A concessão do host
/// estava certa e o wizard dizia "falta", porque a do `logid` era de um
/// binário antigo. É a mesma pergunta que o LaunchAgent do host responde.
fn conferir_fora_do_logid(alvo: &std::path::Path) -> Result<String, String> {
    const ROTULO: &str = "ai.logi.conferir";
    let arquivo = crate::sys::config_dir().join("conferir.out");
    let caminho = arquivo.to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&arquivo);
    let _ = rodar("/bin/launchctl", &["remove", ROTULO], 5);
    rodar(
        "/bin/launchctl",
        &[
            "submit", "-l", ROTULO, "-o", &caminho, "-e", &caminho, "--",
            &alvo.to_string_lossy(), "--conferir",
        ],
        5,
    )?;
    let limite = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut texto = String::new();
    while std::time::Instant::now() < limite {
        texto = std::fs::read_to_string(&arquivo).unwrap_or_default();
        if texto.contains("acessibilidade: ") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = rodar("/bin/launchctl", &["remove", ROTULO], 5);
    let _ = std::fs::remove_file(&arquivo);
    if texto.is_empty() {
        return Err("o host não respondeu em 8 s".into());
    }
    Ok(texto)
}

fn conferir_permissoes() -> (Medida, Medida) {
    let alvo = host_bin();
    if !alvo.exists() {
        let m = |id: &str| Medida::erro(id, "o host de tela não está instalado: não há a quem perguntar");
        return (m("gravacao-de-tela"), m("acessibilidade"));
    }
    // 10 s porque o binário só imprime duas linhas e sai; se passar
    // disso, algo está pendurado e pendurado não é "ok".
    let saida = conferir_fora_do_logid(&alvo);
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

fn tailscale() -> Medida {
    // Só os caminhos do brew. O `/usr/local/bin/tailscale` que o app
    // instala é um script que abre o app, e abrir o app é o que dispara
    // o pedido de VPN: medir por ele muda o que está sendo medido.
    let app = std::path::Path::new("/Applications/Tailscale.app").exists();
    let cli = ["/opt/homebrew/bin/tailscale", "/usr/local/opt/tailscale/bin/tailscale"]
        .into_iter()
        .find(|c| std::path::Path::new(c).exists());
    let Some(cli) = cli else {
        return Medida::falta("tailscale", "tailscale do brew não instalado");
    };
    match rodar(cli, &["status", "--json"], 5) {
        Ok(json) => crate::setup_comum::veredito_tailscale(&json, app),
        Err(e) => Medida::falta("tailscale", format!("o tailscaled não respondeu: {e}")),
    }
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
            None,
        ),
        "launchagent" => instalar_agente(
            "ai.logi.logistream-host",
            &[
                host_bin().to_string_lossy().into_owned(),
                "9777".into(),
                "Capture screen 0".into(),
            ],
            false,
            None,
        ),
        "ota" => instalar_relogio(),
        "tailscale" => instalar_tailscale(),
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
fn instalar_agente(
    rotulo: &str,
    argv: &[String],
    run_at_load: bool,
    intervalo: Option<u64>,
) -> Result<String, String> {
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
         {intervalo}\
         \x20 <key>ProcessType</key><string>Interactive</string>\n\
         \x20 <key>StandardOutPath</key><string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key><string>{log}</string>\n\
         </dict>\n</plist>\n",
        ral = if run_at_load { "true" } else { "false" },
        // `StartInterval` e `RunAtLoad false` juntos: o launchd dispara
        // a primeira vez só depois do intervalo. É o que se quer aqui --
        // disparar no `bootstrap` faria o agente aplicar um build novo e
        // reiniciar o `logid` no meio da requisição que o instalou.
        intervalo = match intervalo {
            Some(s) => format!("  <key>StartInterval</key><integer>{s}</integer>\n"),
            None => String::new(),
        },
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

/// Instala o LaunchAgent do `logi-ota` e confere uma vez.
///
/// Uma hora de intervalo. `RunAtLoad false`: o `bootstrap` não pode
/// disparar o agente agora, porque ele aplicaria o build novo e
/// reiniciaria o `logid` -- que é quem está respondendo esta
/// requisição. A primeira checagem aqui é `--conferir`, que não troca
/// nada; a troca acontece no primeiro tique, sem ninguém esperando.
///
/// Consequência aceita e documentada: depois de um boot, o Mac pode
/// levar até uma hora para olhar. Para quem viaja com o laptop, isso é
/// "quando eu abrir a tampa e for fazer café".
fn instalar_relogio() -> Result<String, String> {
    let bin = bin_dir().join("logi-ota");
    if !bin.is_file() {
        return Err(format!("não achei {}: rode o deploy", bin.display()));
    }
    instalar_agente(
        "ai.logi.ota",
        &[bin.to_string_lossy().into_owned()],
        false,
        Some(3600),
    )?;
    relogio()?;
    let primeira = rodar(&bin.to_string_lossy(), &["--conferir"], 60);
    Ok(match primeira {
        Ok(s) => format!("agente instalado; primeira checagem: {}", s.trim()),
        Err(e) => format!("agente instalado, mas a primeira checagem falhou: {e}"),
    })
}

/// Abre o Terminal com o instalador que vem dentro do binário.
///
/// O `sudo` e o login precisam de gente, e o Terminal é o lugar deles.
/// Máquina nova não tem clone do repo, então os dois arquivos saem daqui.
fn instalar_tailscale() -> Result<String, String> {
    const SCRIPT: &str = include_str!("../setup/instalar-tailscale-mac.sh");
    let dir = crate::sys::config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("criar {dir:?}: {e}"))?;
    let script = dir.join("instalar-tailscale-mac.sh");
    let abrir = dir.join("instalar-tailscale.command");
    std::fs::write(&script, SCRIPT).map_err(|e| format!("gravar {script:?}: {e}"))?;
    let comando = format!(
        "#!/bin/bash\n\
         export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH\n\
         if ! command -v brew >/dev/null; then\n\
         \x20 echo 'falta o Homebrew: https://brew.sh'\n\
         else\n\
         \x20 brew list tailscale >/dev/null 2>&1 || brew install tailscale\n\
         \x20 sudo bash '{}'\n\
         fi\n\
         read -r -p 'Enter para fechar '\n",
        script.display()
    );
    std::fs::write(&abrir, comando).map_err(|e| format!("gravar {abrir:?}: {e}"))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&abrir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("permissão de {abrir:?}: {e}"))?;
    rodar("/usr/bin/open", &[&abrir.to_string_lossy()], 10)?;
    let m = tailscale();
    match m.estado {
        Estado::Ok => Ok(m.detalhe),
        _ => Err(format!("{} -- termine no Terminal que abriu e meça de novo", m.detalhe)),
    }
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
