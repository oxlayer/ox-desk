//! What this machine still lacks for the set to work.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! ## Why this exists
//!
//! Setting up a new machine for this set is a list of ~10 things,
//! different on each OS, and none of them gives a clear error when it is
//! missing: without an ACL on `/dev/hidraw` HID++ just finds no device;
//! without Screen Recording AVFoundation **hangs** instead of failing;
//! without an `/IT` task the Windows host starts in session 0 and
//! captures black. In all these cases the symptom is "it didn't work" and
//! the cause is three commands away.
//!
//! `status.sh` already answered this in the terminal, but only on Linux
//! and only for whoever knows it exists. This module is the same
//! diagnosis with three differences that matter: it runs on all three
//! machines, it runs **inside** the daemon (so it measures its own
//! session, not the SSH one -- see
//! [[../../.claude/rules/graphical-session]]), and it knows how to fix
//! part of what it finds.
//!
//! ## The honesty rule
//!
//! A step that cannot be measured becomes [`State::Error`], **never**
//! [`State::Ok`]. We already had two guards that guarded nothing
//! (`dispatch.backfill_gate` in another project, the langfuse `catch`)
//! and the pattern is always the same: silence looked like success. Here
//! "I don't know" is a visible state, and it blocks "ready" just like
//! "missing" does.
//!
//! After fixing, **measure again**. The button never turns green for
//! having run the command; it turns green for having measured the result
//! ([[../../.claude/rules/verify-what-arrived]]).
//!
//! ## What is pure and what is not
//!
//! This file is the catalog and the decision -- which steps exist on each
//! OS, in what order, who waits for whom, and when the set is ready.
//! Nothing here touches disk, network or processes, so the tests run on
//! the development laptop covering all **three** catalogs, not only this
//! machine's. That is what caught the dangling dependency in
//! `catalog(Windows)` before it existed on a machine.
//!
//! The real measurement and the repairs live in `setup_linux.rs`,
//! `setup_mac.rs` and `setup_win.rs`.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum System {
    Linux,
    Mac,
    Windows,
}

impl System {
    pub fn this_machine() -> Self {
        if cfg!(target_os = "macos") {
            System::Mac
        } else if cfg!(windows) {
            System::Windows
        } else {
            System::Linux
        }
    }
}

/// How this step gets fixed -- and who has to act.
///
/// The distinction between `Sudo` and `Click` is not cosmetic: the first
/// one the person solves by pasting a line into any terminal, even over
/// SSH; the second **only** works in front of the machine, because it is
/// macOS TCC or Windows UAC asking for human consent. Mixing the two is
/// promising automation that does not exist.
///
/// The wire names (`tipo`, `sozinho`, `rotulo`, ...) are read by
/// `static/setup.html`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "tipo", rename_all = "lowercase")]
pub enum Fix {
    /// The daemon does it now, on its own, and measures again.
    #[serde(rename = "sozinho")]
    Auto {
        #[serde(rename = "rotulo")]
        label: &'static str,
    },
    /// Needs root. We show the exact line to copy.
    Sudo {
        #[serde(rename = "linha")]
        line: &'static str,
    },
    /// Needs a human click in a system window. The daemon opens the
    /// right window; the person decides.
    #[serde(rename = "clique")]
    Click {
        #[serde(rename = "rotulo")]
        label: &'static str,
        #[serde(rename = "onde")]
        location: &'static str,
    },
    /// There is no honest automation here. The recipe is written down.
    #[serde(rename = "receita")]
    Recipe {
        #[serde(rename = "linha")]
        line: &'static str,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: &'static str,
    #[serde(rename = "titulo")]
    pub title: &'static str,
    /// One line saying what breaks without this.
    ///
    /// It exists because "enable this" without a reason becomes an
    /// automatic click, and three months later not even I remember why
    /// the step is on the list.
    #[serde(rename = "porque")]
    pub why: &'static str,
    #[serde(rename = "arrumo")]
    pub fix: Fix,
    /// Ids that must be `Ok` before this one makes sense.
    #[serde(rename = "depende")]
    pub depends_on: &'static [&'static str],
    /// `false` = the set works without this, it only loses a part.
    #[serde(rename = "essencial")]
    pub essential: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    #[serde(rename = "falta")]
    Missing,
    /// Measuring failed. It is not "ok" and not "missing": it is "I don't
    /// know".
    #[serde(rename = "erro")]
    Error,
    /// Does not apply to this machine (no GPU, no receiver).
    #[serde(rename = "fora")]
    NotApplicable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: String,
    #[serde(rename = "estado")]
    pub state: State,
    #[serde(rename = "detalhe")]
    pub detail: String,
}

impl Check {
    pub fn ok(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::Ok, detail: detail.into() }
    }
    pub fn missing(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::Missing, detail: detail.into() }
    }
    pub fn error(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::Error, detail: detail.into() }
    }
    pub fn not_applicable(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::NotApplicable, detail: detail.into() }
    }
    /// Shortcut for a boolean measurement, which is most of them.
    pub fn when(id: &str, cond: bool, good: impl Into<String>, bad: impl Into<String>) -> Self {
        if cond {
            Check::ok(id, good)
        } else {
            Check::missing(id, bad)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    /// Essential steps that apply to this machine.
    pub total: usize,
    #[serde(rename = "prontos")]
    pub done: usize,
    /// Every applicable essential step is `Ok`.
    #[serde(rename = "pronto")]
    pub ready: bool,
    /// First step where acting makes a difference now.
    #[serde(rename = "proximo")]
    pub next: Option<String>,
    /// Steps that fail but depend on another one that fails earlier.
    #[serde(rename = "esperando")]
    pub waiting: Vec<String>,
}

pub fn state_of(checks: &[Check], id: &str) -> State {
    // Absent = Error, not Ok. A step nobody measured did not pass:
    // forgetting to measure is exactly how a "ready" lies.
    checks.iter().find(|m| m.id == id).map(|m| m.state).unwrap_or(State::Error)
}

/// Which dependency of this step is not up yet.
///
/// Returns the **first** one -- catalog order is resolution order, so
/// the first is the one the person should tackle.
pub fn blocked_by(p: &Step, checks: &[Check]) -> Option<String> {
    p.depends_on
        .iter()
        .find(|d| !matches!(state_of(checks, d), State::Ok | State::NotApplicable))
        .map(|d| (*d).to_string())
}

pub fn summarize(steps: &[Step], checks: &[Check]) -> Summary {
    let mut total = 0;
    let mut done = 0;
    let mut ready = true;
    let mut next = None;
    let mut waiting = Vec::new();

    for p in steps {
        let e = state_of(checks, p.id);
        if e == State::NotApplicable {
            // Does not apply: it neither counts nor delays anyone.
            // Counting it as pending would make the panel complain about
            // a missing GPU on a machine that will never have one.
            continue;
        }
        if p.essential {
            total += 1;
            if e == State::Ok {
                done += 1;
            } else {
                ready = false;
            }
        }
        if e != State::Ok {
            match blocked_by(p, checks) {
                Some(_) => waiting.push(p.id.to_string()),
                None if next.is_none() => next = Some(p.id.to_string()),
                None => {}
            }
        }
    }

    Summary { total, done, ready, next, waiting }
}

// ---------------------------------------------------------------- catalogs

/// This machine's steps, in resolution order.
pub fn local_catalog() -> Vec<Step> {
    catalog(System::this_machine())
}

pub fn catalog(s: System) -> Vec<Step> {
    match s {
        System::Linux => linux(),
        System::Mac => mac(),
        System::Windows => windows(),
    }
}

fn linux() -> Vec<Step> {
    vec![
        Step {
            id: "binarios",
            title: "Binários instalados",
            why: "o daemon, o host de tela e o KVM vivem em ~/.local/bin. \
                     Faltou o logistream-kvm uma vez e o botão da bandeja \
                     falhava com \"não consegui executar\" -- compilado, \
                     nunca instalado.",
            fix: Fix::Recipe { line: "bash install.sh   # ou ./scripts/deploy.sh" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "config",
            title: "Config desta máquina",
            why: "cada máquina tem a própria config e nada sincroniza. \
                     Sem a entrada desta máquina marcada como local, o \
                     painel não sabe quem é \"aqui\".",
            fix: Fix::Auto { label: "Escrever a config padrão" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "daemon",
            title: "Serviço de usuário ativo",
            why: "o logid.service é quem sobe o painel no logon. Rodando \
                     na mão funciona até a primeira vez que você reinicia.",
            fix: Fix::Auto { label: "Habilitar e iniciar" },
            depends_on: &["binarios"],
            essential: true,
        },
        Step {
            id: "receptor",
            title: "Acesso ao receptor (HID++)",
            why: "é por /dev/hidraw que trocamos o canal Easy-Switch do \
                     teclado e do mouse. Sem acesso o painel simplesmente \
                     não acha dispositivo -- não diz \"sem permissão\".",
            fix: Fix::Sudo {
                line: "sudo install -m644 ~/dev/logi/logid/udev/70-logid-hidraw.rules /etc/udev/rules.d/\n\
                        sudo udevadm control --reload\n\
                        sudo udevadm trigger --subsystem-match=hidraw --action=add",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "input-kvm",
            title: "Acesso ao teclado e mouse (KVM por LAN)",
            why: "o KVM lê /dev/input/event* e chama EVIOCGRAB. A regra usa \
                     TAG+=\"uaccess\" restrito ao vendor 046d, e NÃO o grupo \
                     input: o grupo daria a qualquer processo da conta acesso \
                     permanente a todos os dispositivos de entrada -- um \
                     keylogger passivo de graça.",
            fix: Fix::Sudo {
                line: "sudo install -m644 ~/dev/logi/logid/udev/70-logid-input.rules /etc/udev/rules.d/\n\
                        sudo udevadm control --reload\n\
                        sudo udevadm trigger --subsystem-match=input",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "wayland",
            title: "Sessão Wayland",
            why: "a detecção de borda usa a API privada do Mutter \
                     (org.gnome.Mutter.InputCapture), e o clipboard usa a \
                     RemoteDesktop do mesmo Mutter. No X11 nenhum dos dois \
                     foi validado.",
            fix: Fix::Recipe { line: "escolha \"GNOME\" (não \"GNOME on Xorg\") na tela de login" },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "clipboard",
            title: "Clipboard entre as máquinas",
            why: "o Mutter precisa aceitar a sessão de RemoteDesktop para \
                     observarmos a área de transferência. Sem isso a cópia \
                     não atravessa.",
            fix: Fix::Auto { label: "Testar agora" },
            depends_on: &["daemon"],
            essential: true,
        },
        Step {
            id: "pares",
            title: "SSH alcança as outras máquinas",
            why: "SSH é o transporte de tudo que cruza máquina -- troca de \
                     tela, clipboard, arquivos. Sem chave autorizada o painel \
                     mostra as outras como mortas.",
            fix: Fix::Recipe { line: "ssh-copy-id <alias>   # alias é o campo `ssh` de cada host na config" },
            depends_on: &["config"],
            essential: true,
        },
        Step {
            id: "nvenc",
            title: "Codificador de vídeo por hardware",
            why: "esta máquina só pode ser ASSISTIDA se tiver NVENC. Sem \
                     ele o painel cai no caminho de software, que é mais \
                     lento e esquenta.",
            fix: Fix::Recipe { line: "driver NVIDIA proprietário instalado; confira com nvidia-smi" },
            depends_on: &["binarios"],
            essential: false,
        },
        Step {
            id: "bandeja",
            title: "Ícone na bandeja",
            why: "é o caminho mais curto para trazer o teclado de volta \
                     quando ele está em outra máquina. Sem ele você depende \
                     de lembrar os dois Ctrl.",
            fix: Fix::Auto { label: "Lançar agora" },
            depends_on: &["daemon"],
            essential: false,
        },
        Step {
            id: "ota",
            title: "Atualização automática (OTA)",
            why: "esta máquina se atualiza sozinha pelas Releases do \
                     GitHub -- é o que mantém o Mac em dia quando ele \
                     está longe de casa e ninguém pode rodar o deploy. \
                     Sem o relógio, o build velho fica.",
            fix: Fix::Auto { label: "Instalar o relógio e conferir agora" },
            depends_on: &["binarios"],
            essential: false,
        },
    ]
}

fn mac() -> Vec<Step> {
    vec![
        Step {
            id: "binarios",
            title: "Binários instalados",
            why: "o Mac é cliente E host de tela. Deixar o host para trás \
                     já aconteceu: a máquina assistia as outras e nunca era \
                     assistida, e o painel caía no VNC sem dizer por quê.",
            fix: Fix::Recipe { line: "./scripts/deploy.sh   # compila no próprio Mac, sem cross-compile" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "config",
            title: "Config desta máquina",
            why: "cada máquina tem a própria config e nada sincroniza.",
            fix: Fix::Auto { label: "Escrever a config padrão" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "daemon",
            title: "LaunchAgent do daemon",
            why: "o logid tem de nascer na sessão gráfica. Lançado por SSH \
                     ele não enxerga nem o chaveiro nem as permissões.",
            fix: Fix::Auto { label: "Instalar e carregar" },
            depends_on: &["binarios"],
            essential: true,
        },
        Step {
            id: "ffmpeg",
            title: "ffmpeg que os binários pedem",
            why: "o Mac é a única máquina que liga no ffmpeg -- captura \
                     por avfoundation, codificação por h264_videotoolbox -- \
                     e o soname entra NO BINÁRIO: compilado contra o major \
                     9 ele não carrega com o major 7 instalado. Sem a \
                     formula certa a tela não sobe e o OTA recusa o update \
                     inteiro: está certo, mas chega como recusa de hora em \
                     hora, e não como uma linha aqui.",
            fix: Fix::Recipe { line: "brew install ffmpeg@7" },
            depends_on: &["binarios"],
            essential: true,
        },
        Step {
            id: "identidade",
            title: "Identidade local de assinatura",
            why: "sem ela o binário é ad-hoc, a amarração do TCC é pelo \
                     cdhash e a permissão de tela MORRE a cada compilação. \
                     Medido: conceder, rodar um deploy, e o host voltar a \
                     travar sem erro nenhum no log.",
            fix: Fix::Recipe {
                line: "bash ~/.local/bin/criar-identidade.sh   # no Terminal do Mac: por SSH o chaveiro está trancado",
            },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "assinatura",
            title: "Host de tela assinado",
            why: "é o que faz a permissão sobreviver ao próximo deploy. \
                     Conceder a permissão ANTES de assinar desperdiça a \
                     concessão -- e só o dono da máquina pode dar de volta, \
                     à mão.",
            fix: Fix::Auto { label: "Assinar agora" },
            depends_on: &["binarios", "identidade"],
            essential: true,
        },
        Step {
            id: "gravacao-de-tela",
            title: "Permissão de Gravação de Tela",
            why: "sem ela o AVFoundation TRAVA em vez de falhar: o log \
                     para em \"abrindo a tela 1\" e a última linha é uma \
                     mensagem do ffmpeg sobre formato de pixel, que não tem \
                     nada a ver com o problema. Cuidado: a PRIMEIRA \
                     abertura depois de trocar o binário levou ~30 s, com o \
                     log parado nessa mesma frase (as seguintes, ~3 s). \
                     Espere 30 s antes de culpar o TCC.",
            fix: Fix::Click {
                label: "Pedir a permissão",
                location: "Ajustes > Privacidade e Segurança > Gravação de Tela > logistream-host",
            },
            depends_on: &["assinatura"],
            essential: true,
        },
        Step {
            id: "acessibilidade",
            title: "Permissão de Acessibilidade",
            why: "é o que permite injetar teclado e mouse. Sem ela a tela \
                     aparece e o ponteiro não anda -- mesmo sintoma de \"não \
                     funcionou\", causa diferente.",
            fix: Fix::Click {
                label: "Pedir a permissão",
                location: "Ajustes > Privacidade e Segurança > Acessibilidade > logistream-host",
            },
            depends_on: &["assinatura"],
            essential: true,
        },
        Step {
            id: "launchagent",
            title: "LaunchAgent do host de tela",
            why: "o host sobe por launchctl na sessão gráfica, nunca por \
                     SSH. RunAtLoad false de propósito: capturar sem ninguém \
                     assistindo gastaria bateria à toa.",
            fix: Fix::Auto { label: "Instalar e carregar" },
            depends_on: &["binarios"],
            essential: true,
        },
        Step {
            id: "clipboard",
            title: "Clipboard entre as máquinas",
            why: "a leitura do clipboard do Mac passa por JXA, e isso só \
                     roda na sessão gráfica.",
            fix: Fix::Auto { label: "Testar agora" },
            depends_on: &["daemon"],
            essential: true,
        },
        Step {
            id: "login-remoto",
            title: "Login Remoto (SSH de entrada)",
            why: "é o que deixa o deploy e o painel alcançarem este Mac. \
                     Sem isso ele só fala, nunca escuta.",
            fix: Fix::Sudo { line: "sudo systemsetup -f -setremotelogin on" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "tailscale",
            title: "Tailscale (fora da LAN)",
            why: "é o que alcança as outras máquinas fora de casa. Vai pelo \
                     brew, com serviço próprio, e nunca pelo app: a extensão \
                     de rede do app, sem login, derrubou a internet do Mac.",
            fix: Fix::Click {
                label: "Instalar pelo brew",
                location: "o Terminal que vai abrir: digite a senha e abra o link de login",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "pares",
            title: "SSH alcança as outras máquinas",
            why: "SSH é o transporte de tudo que cruza máquina.",
            fix: Fix::Recipe { line: "ssh-copy-id <alias>" },
            depends_on: &["config"],
            essential: true,
        },
        Step {
            id: "ota",
            title: "Atualização automática (OTA)",
            why: "é o passo que faz este Mac se manter em dia longe de \
                     casa. O agente baixa o build publicado, **assina com a \
                     identidade local** e só então troca -- é isso que \
                     preserva a Gravação de Tela, que um binário ad-hoc da \
                     CI perderia.",
            fix: Fix::Auto { label: "Instalar o relógio e conferir agora" },
            depends_on: &["binarios", "identidade"],
            essential: false,
        },
    ]
}

fn windows() -> Vec<Step> {
    vec![
        Step {
            id: "binarios",
            title: "Binários instalados",
            why: "o daemon, o agente, o host e o visualizador vivem em \
                     %LOCALAPPDATA%\\logid. O deploy assina cada um -- sem \
                     assinatura o Defender come o executável sem avisar.",
            fix: Fix::Recipe { line: "./scripts/deploy.sh   # do Ubuntu: cross-compile, assina e troca" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "config",
            title: "Config desta máquina",
            why: "cada máquina tem a própria config e nada sincroniza.",
            fix: Fix::Auto { label: "Escrever a config padrão" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "daemon",
            title: "Tarefa de logon do daemon",
            why: "no Windows o logid sobe por tarefa agendada com /IT, não \
                     por serviço: serviço nasce na sessão 0 e de lá não se vê \
                     teclado, mouse nem área de transferência.",
            fix: Fix::Auto { label: "Criar a tarefa" },
            depends_on: &["binarios"],
            essential: true,
        },
        Step {
            id: "tarefa-tela",
            title: "Tarefa interativa do host de tela",
            why: "mesma fronteira: sem /IT o DXGI devolve E_ACCESSDENIED \
                     ou captura preto, e o sintoma não diz nada sobre sessão.",
            fix: Fix::Auto { label: "Criar a tarefa" },
            depends_on: &["binarios"],
            essential: true,
        },
        Step {
            id: "clipboard",
            title: "Clipboard entre as máquinas",
            why: "ler a área de transferência exige estar na sessão \
                     interativa. Pelo SSH se lê a sessão 0, que está sempre \
                     vazia -- e \"vazio\" parece \"nada copiado\".",
            fix: Fix::Auto { label: "Testar agora" },
            depends_on: &["daemon"],
            essential: true,
        },
        Step {
            id: "sshd",
            title: "Servidor OpenSSH",
            why: "é por onde o Ubuntu manda deploy, clipboard e arquivos. \
                     Sem ele esta máquina some do painel.",
            fix: Fix::Sudo {
                line: "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0; Set-Service sshd -StartupType Automatic; Start-Service sshd",
            },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "chave-admin",
            title: "Chave autorizada",
            why: "o OpenSSH do Windows lê a chave de um administrador em \
                     ProgramData\\ssh\\administrators_authorized_keys, NÃO no \
                     ~/.ssh do usuário. Põr no lugar errado falha com \
                     \"Permission denied\" e nenhuma pista.",
            fix: Fix::Recipe {
                line: "ssh-copy-id não existe aqui: ver .claude/rules/remote-powershell.md",
            },
            depends_on: &["sshd"],
            essential: true,
        },
        Step {
            id: "pares",
            title: "SSH alcança as outras máquinas",
            why: "SSH é o transporte de tudo que cruza máquina.",
            fix: Fix::Recipe { line: "ssh-keygen e copiar a pública para o outro lado" },
            depends_on: &["config"],
            essential: true,
        },
        Step {
            id: "tela-virtual",
            title: "Tela sem monitor ligado",
            why: "sem monitor o Windows não tem o que capturar: o visualizador \
                     conecta, o clique passa e nenhum quadro chega. Uma tela \
                     virtual mantém o acesso com a máquina sozinha.",
            fix: Fix::Click {
                label: "Instalar a tela virtual",
                location: "o pedido de administrador do Windows",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "protetor-de-tela",
            title: "Protetor de tela e bloqueio",
            why: "com a sessão bloqueada o DXGI devolve E_ACCESSDENIED e a \
                     tela remota morre sozinha depois de alguns minutos. A \
                     escolha é sua: desligar o bloqueio, ou saber que a tela \
                     cai quando ele entra.",
            fix: Fix::Recipe {
                line: "powercfg /setacvalueindex SCHEME_CURRENT SUB_NONE CONSOLELOCK 0",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "ota",
            title: "Atualização automática (OTA)",
            why: "aqui o OTA **só confere e relata**: a chave do \
                     Authenticode não sai do Ubuntu, e `.exe` sem assinatura \
                     válida é o que o Defender apaga na escrita. Serve para \
                     o painel dizer quando esta máquina ficou atrás -- quem \
                     troca é o deploy.",
            fix: Fix::Auto { label: "Instalar a tarefa e conferir agora" },
            depends_on: &["binarios"],
            essential: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_THREE: [System; 3] = [System::Linux, System::Mac, System::Windows];

    fn m(id: &str, e: State) -> Check {
        Check { id: id.into(), state: e, detail: String::new() }
    }

    #[test]
    fn error_does_not_count_as_ok() {
        // The point of the honesty rule: "I can't measure" must weigh the
        // same as "missing". If it weighed less, the hardest step to
        // measure would be the easiest to pass.
        let steps = linux();
        let checks: Vec<_> = steps.iter().map(|p| m(p.id, State::Error)).collect();
        let r = summarize(&steps, &checks);
        assert!(!r.ready);
        assert_eq!(r.done, 0);
    }

    #[test]
    fn step_without_check_does_not_pass() {
        // Forgetting to measure a step is the easiest way for a "ready"
        // to lie -- and the most likely one, because the catalog and the
        // measurement live in different files.
        let steps = linux();
        let checks: Vec<_> = steps
            .iter()
            .filter(|p| p.id != "clipboard")
            .map(|p| m(p.id, State::Ok))
            .collect();
        let r = summarize(&steps, &checks);
        assert!(!r.ready);
        assert_eq!(state_of(&checks, "clipboard"), State::Error);
    }

    #[test]
    fn not_applicable_neither_blocks_nor_counts() {
        let steps = linux();
        let total_before = {
            let all: Vec<_> = steps.iter().map(|p| m(p.id, State::Ok)).collect();
            summarize(&steps, &all).total
        };
        let checks: Vec<_> = steps
            .iter()
            .map(|p| m(p.id, if p.id == "clipboard" { State::NotApplicable } else { State::Ok }))
            .collect();
        let r = summarize(&steps, &checks);
        assert!(r.ready, "an out-of-scope step does not prevent ready");
        assert_eq!(r.total, total_before - 1, "and it does not count either");
        assert_eq!(r.next, None);
    }

    #[test]
    fn non_essential_does_not_prevent_ready() {
        let steps = linux();
        let checks: Vec<_> = steps
            .iter()
            .map(|p| m(p.id, if p.essential { State::Ok } else { State::Missing }))
            .collect();
        let r = summarize(&steps, &checks);
        assert!(r.ready);
        // But it still shows up as the next action: not blocking is not
        // hiding.
        assert!(r.next.is_some());
    }

    #[test]
    fn next_skips_steps_waiting_on_a_dependency() {
        // `daemon` depends on `binarios`. With both missing, the offered
        // action must be installing the binaries -- telling the person to
        // enable a service whose executable does not exist fixes nothing
        // and teaches them to distrust the wizard.
        let steps = linux();
        let checks: Vec<_> = steps
            .iter()
            .map(|p| {
                let missing = p.id == "daemon" || p.id == "binarios";
                m(p.id, if missing { State::Missing } else { State::Ok })
            })
            .collect();
        let r = summarize(&steps, &checks);
        assert_eq!(r.next.as_deref(), Some("binarios"));
        assert!(r.waiting.contains(&"daemon".to_string()));
    }

    #[test]
    fn out_of_scope_dependency_releases_its_dependents() {
        // `NotApplicable` is not a failure: if the dependency does not
        // apply to this machine, whoever waits for it must be able to move.
        let p = Step {
            id: "x",
            title: "",
            why: "",
            fix: Fix::Auto { label: "" },
            depends_on: &["y"],
            essential: true,
        };
        assert_eq!(blocked_by(&p, &[m("y", State::NotApplicable)]), None);
        assert_eq!(blocked_by(&p, &[m("y", State::Missing)]).as_deref(), Some("y"));
    }

    #[test]
    fn all_ok_is_ready() {
        for s in ALL_THREE {
            let steps = catalog(s);
            let checks: Vec<_> = steps.iter().map(|p| m(p.id, State::Ok)).collect();
            let r = summarize(&steps, &checks);
            assert!(r.ready, "{s:?}");
            assert_eq!(r.next, None, "{s:?}");
            assert_eq!(r.done, r.total, "{s:?}");
            assert!(r.total > 0, "{s:?}");
        }
    }

    #[test]
    fn ids_unique_in_each_catalog() {
        for s in ALL_THREE {
            let steps = catalog(s);
            let mut ids: Vec<_> = steps.iter().map(|p| p.id).collect();
            let before = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), before, "duplicate id in {s:?}");
        }
    }

    #[test]
    fn dependency_exists_and_comes_first() {
        // A dangling dependency would leave the step waiting forever, and
        // the wizard would never offer any action -- a "this is missing"
        // screen with no button. A dependency pointing FORWARD would show
        // "waiting" above the thing waited for, which reads bottom-up.
        for s in ALL_THREE {
            let steps = catalog(s);
            for (i, p) in steps.iter().enumerate() {
                for d in p.depends_on {
                    let pos = steps.iter().position(|q| q.id == *d);
                    let pos = pos.unwrap_or_else(|| panic!("{s:?}: {} depends on {d}, which does not exist", p.id));
                    assert!(pos < i, "{s:?}: {} depends on {d}, which comes later", p.id);
                }
            }
        }
    }

    #[test]
    fn every_step_says_why() {
        // A step without a reason becomes an automatic click. Cheap to
        // test, and the test is what stops haste from slipping in a
        // `why: ""`.
        for s in ALL_THREE {
            for p in catalog(s) {
                assert!(p.why.len() > 40, "{s:?}: {} has no written reason", p.id);
                assert!(!p.title.is_empty(), "{s:?}: {} has no title", p.id);
            }
        }
    }
}

// ---------------------------------------------------------- per-OS dispatch
//
// Only here does the pure module touch the world, and only by
// delegation: the real measurement lives in the platform file. The
// alternative -- `cfg` scattered through the catalog -- would make the
// tests above cover only the OS being compiled, which is exactly what
// they exist to avoid.

/// Measures this machine's steps, in catalog order.
pub fn measure(cfg: &crate::config::Config) -> Vec<Check> {
    #[cfg(target_os = "linux")]
    return crate::setup_linux::measure(cfg);
    #[cfg(target_os = "macos")]
    return crate::setup_mac::measure(cfg);
    #[cfg(windows)]
    return crate::setup_win::measure(cfg);
}

/// Tries to fix a step and returns **what it measured afterwards**.

pub fn repair(id: &str, cfg: &crate::config::Config) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    return crate::setup_linux::repair(id, cfg);
    #[cfg(target_os = "macos")]
    return crate::setup_mac::repair(id, cfg);
    #[cfg(windows)]
    return crate::setup_win::repair(id, cfg);
}
