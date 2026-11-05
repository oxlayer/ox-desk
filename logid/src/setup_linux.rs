//! Medição e reparo do setup, no Ubuntu.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! O catálogo e a decisão ficam em `setup.rs`; aqui só se mede o mundo e
//! se mexe nele. Duas coisas que a divisão impõe, e que valem:
//!
//! 1. Toda medição é **barata e com teto de tempo**. O wizard chama
//!    `medir()` a cada poucos segundos; um passo que leva 10 s derruba o
//!    painel inteiro. Onde a medida honesta é caro (NVENC, SSH), ela sai
//!    de cache ou de um probe com timeout.
//! 2. Todo reparo volta a mensagem do que **mediu depois**, não do que
//!    mandou. "Rodei o comando" não é resultado
//!    ([[../../.claude/rules/verificar-o-que-chegou]]).

use crate::config::Config;
use crate::setup::{Estado, Medida};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const BINS: [&str; 4] = ["logid", "logistream-host", "logistream-view", "logistream-kvm"];

fn bin_dir() -> PathBuf {
    crate::sys::home_dir().join(".local/bin")
}

pub fn medir(cfg: &Config) -> Vec<Medida> {
    vec![
        binarios(),
        crate::setup_comum::config_desta_maquina(cfg),
        daemon(),
        receptor(),
        input_kvm(),
        wayland(),
        crate::setup_comum::clipboard(),
        crate::setup_comum::pares(cfg),
        nvenc(),
        bandeja(),
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
    let estado = crate::sys::unit_state("logid.service");
    let habilitado = Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", "logid.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    match (estado.as_str(), habilitado) {
        ("active", true) => Medida::ok("daemon", "ativo e habilitado no logon"),
        // Ativo mas não habilitado é o caso traiçoeiro: funciona hoje e
        // desaparece no próximo boot. Tem de pesar como falta.
        ("active", false) => Medida::falta("daemon", "ativo, mas NÃO habilitado: não volta no próximo logon"),
        (outro, _) => Medida::falta("daemon", format!("unit {outro}")),
    }
}

/// Alguma regra de udev cobre `hidraw` dos dispositivos Logitech?
///
/// Varredura em NÍVEL DE ARQUIVO, não de linha, e os três diretórios.
/// A primeira versão disto olhava só `/etc/udev/rules.d` e exigia
/// `hidraw` e `046d` na MESMA linha -- e dizia "nenhuma regra" numa
/// máquina onde o acesso funcionava. A regra real (a do pacote do
/// Solaar, em `/usr/lib`) usa `GOTO`, então o subsistema e o vendor
/// aparecem em linhas diferentes. Medição errada, máquina certa.
fn regra_de_hidraw() -> Option<String> {
    for dir in ["/etc/udev/rules.d", "/usr/lib/udev/rules.d", "/lib/udev/rules.d"] {
        let Ok(entradas) = std::fs::read_dir(dir) else { continue };
        for e in entradas.flatten() {
            let Ok(texto) = std::fs::read_to_string(e.path()) else { continue };
            let util: String = texto
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with('#') && !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if util.contains("hidraw")
                && util.contains("046d")
                && (util.contains("uaccess") || util.contains("MODE"))
            {
                return Some(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// (nós `hidraw` da Logitech, quantos abrem).
///
/// Filtra pelo vendor em vez de usar `sys::hidraw_contagem`, que conta
/// TODOS os nós: nesta máquina são quatro, e um deles é um dispositivo
/// de outro fabricante que não abre -- então a contagem geral dizia "3
/// de 4" e deixava no ar a ideia de que falta permissão em algum lugar.
/// O vendor sai do `HID_ID` no uevent (`0003:0000046D:...`), que existe
/// tanto para USB quanto para Bluetooth.
fn hidraw_logitech() -> (usize, usize) {
    let Ok(entradas) = std::fs::read_dir("/dev") else { return (0, 0) };
    let mut total = 0;
    let mut abriveis = 0;
    for e in entradas.flatten() {
        let nome = e.file_name();
        let Some(n) = nome.to_str() else { continue };
        if !n.starts_with("hidraw") {
            continue;
        }
        let uevent = std::fs::read_to_string(format!("/sys/class/hidraw/{n}/device/uevent"))
            .unwrap_or_default()
            .to_ascii_uppercase();
        let logitech = uevent
            .lines()
            .filter_map(|l| l.strip_prefix("HID_ID="))
            .any(|v| v.split(':').nth(1).is_some_and(|ven| ven.ends_with("046D")));
        if !logitech {
            continue;
        }
        total += 1;
        if std::fs::File::open(e.path()).is_ok() {
            abriveis += 1;
        }
    }
    (total, abriveis)
}

/// O receptor HID++ está acessível AGORA, e continuará depois de replugar?
///
/// Duas perguntas numa linha de propósito. Só a ACL responderia "funciona
/// hoje" e esconderia que ela morre no próximo replug; só a regra
/// responderia "deveria funcionar" sem olhar se funciona.
fn receptor() -> Medida {
    let (total, abriveis) = hidraw_logitech();
    if total == 0 {
        // Zero nós é ausência de hardware, não de permissão. O painel
        // já mandou a pessoa "rodar udevadm" quando o receptor estava
        // na outra máquina -- consertar uma permissão que não está
        // quebrada.
        return Medida::fora("receptor", "nenhum hidraw Logitech agora: receptor desplugado?");
    }
    let regra = regra_de_hidraw();
    match (abriveis > 0, regra) {
        (true, Some(r)) => Medida::ok("receptor", format!("{abriveis} de {total} nós abrem; regra {r}")),
        (true, None) => Medida::falta(
            "receptor",
            format!("{abriveis} de {total} nós abrem AGORA, mas nenhuma regra cobre hidraw+046d: \
                     a ACL some no próximo replug"),
        ),
        (false, Some(r)) => Medida::falta(
            "receptor",
            format!("{r} existe e nenhum dos {total} nós abre: falta um udevadm trigger"),
        ),
        (false, None) => Medida::falta("receptor", format!("nenhum dos {total} nós de /dev/hidraw abre")),
    }
}

/// Os `event*` dos dispositivos Logitech são legíveis?
///
/// Pelo vendor em `/sys`, não pelo nome: "MX Master" vira outro nome
/// quando o dispositivo troca de canal, e um filtro por nome mediria
/// certo hoje e errado amanhã.
fn input_kvm() -> Medida {
    let Ok(entradas) = std::fs::read_dir("/dev/input") else {
        return Medida::erro("input-kvm", "não consegui ler /dev/input");
    };
    let mut total = 0;
    let mut legiveis = 0;
    for e in entradas.flatten() {
        let nome = e.file_name();
        let Some(n) = nome.to_str() else { continue };
        if !n.starts_with("event") {
            continue;
        }
        let vendor = std::fs::read_to_string(format!("/sys/class/input/{n}/device/id/vendor"))
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if vendor != "046d" {
            continue;
        }
        total += 1;
        if std::fs::File::open(e.path()).is_ok() {
            legiveis += 1;
        }
    }
    if total == 0 {
        return Medida::fora("input-kvm", "nenhum dispositivo Logitech em /dev/input agora");
    }
    Medida::se(
        "input-kvm",
        legiveis == total,
        format!("{legiveis} de {total} dispositivos Logitech legíveis"),
        format!("só {legiveis} de {total} dispositivos Logitech são legíveis"),
    )
}

fn wayland() -> Medida {
    match std::env::var("XDG_SESSION_TYPE").unwrap_or_default().as_str() {
        "wayland" => Medida::ok("wayland", "Wayland"),
        "" => Medida::erro("wayland", "XDG_SESSION_TYPE vazio: o daemon não herdou o ambiente da sessão"),
        outro => Medida::falta("wayland", format!("sessão {outro}: borda e clipboard não foram validados aqui")),
    }
}

fn nvenc() -> Medida {
    // O probe de verdade (`logistream-probe-nvenc`) inicializa CUDA e
    // leva segundos. Para o painel basta a pergunta anterior: existe
    // driver NVIDIA carregado? Sem ele o probe falha de qualquer jeito,
    // e com ele o caminho de tela já foi exercitado de fato pelo uso.
    if !std::path::Path::new("/dev/nvidiactl").exists() {
        return Medida::fora("nvenc", "sem GPU NVIDIA: esta máquina pode assistir, não ser assistida");
    }
    match crate::setup_comum::rodar("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"], 5) {
        Ok(s) => {
            let nome = s.lines().next().unwrap_or("").trim().to_string();
            Medida::ok("nvenc", if nome.is_empty() { "driver carregado".into() } else { nome })
        }
        Err(e) => Medida::erro("nvenc", format!("nvidia-smi: {e}")),
    }
}

fn bandeja() -> Medida {
    // `pgrep -f` no padrão que a skill de deploy usa para matar o
    // processo. Casar pelo nome "electron" sozinho pegaria qualquer app
    // Electron da máquina.
    let viva = Command::new("pgrep")
        .args(["-f", "electron \\.$"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    Medida::se("bandeja", viva, "rodando", "não está rodando")
}

// ------------------------------------------------------------------ reparos

pub fn arrumar(id: &str, cfg: &Config) -> Result<String, String> {
    match id {
        "config" => crate::setup_comum::escrever_config(),
        "daemon" => {
            // `enable --now` numa unit que já roda é idempotente, e é o
            // que arruma o caso "ativo mas não habilitado" sem derrubar
            // o daemon que está respondendo esta requisição.
            crate::setup_comum::rodar(
                "systemctl",
                &["--user", "enable", "--now", "logid.service"],
                15,
            )?;
            Ok("unit habilitada".into())
        }
        "clipboard" => crate::setup_comum::testar_clipboard(),
        "bandeja" => lancar_bandeja(),
        _ => Err(format!("'{id}' não se arruma sozinho nesta máquina")),
    }
    .map(|s| {
        let _ = cfg;
        s
    })
}

/// Lança a casca Electron.
///
/// `env -u ELECTRON_RUN_AS_NODE` não é opcional e não é supérstição:
/// com essa variável no ambiente o electron roda como node puro, `app`
/// vem `undefined` e o processo morre em `requestSingleInstanceLock()`
/// -- com uma mensagem que não diz nada sobre a variável. Já derrubei a
/// GUI do usuário assim, e o daemon herda o ambiente de quem o lançou,
/// então aqui a variável pode muito bem estar presente.
fn lancar_bandeja() -> Result<String, String> {
    let dir = crate::sys::home_dir().join("dev/logi/gui");
    if !dir.join("main.js").exists() {
        return Err(format!("não achei a casca em {}", dir.display()));
    }
    let log = crate::sys::state_dir().join("logi-gui.log");
    let saida = std::fs::File::create(&log).map_err(|e| format!("abrir {}: {e}", log.display()))?;
    let erro = saida.try_clone().map_err(|e| format!("duplicar o log: {e}"))?;
    Command::new("setsid")
        .args(["env", "-u", "ELECTRON_RUN_AS_NODE", "node", "./node_modules/.bin/electron", "."])
        .current_dir(&dir)
        .stdout(saida)
        .stderr(erro)
        .spawn()
        .map_err(|e| format!("lançar: {e}"))?;

    // Medir depois de lançar, não anunciar o lançamento: o electron
    // morre em ~200 ms quando morre, e um "pronto" imediato esconderia
    // justamente a falha mais comum.
    std::thread::sleep(Duration::from_millis(1500));
    if bandeja().estado == Estado::Ok {
        Ok("bandeja no ar".into())
    } else {
        Err(format!("lancei e não ficou de pé; veja {}", log.display()))
    }
}
