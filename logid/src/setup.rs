//! O que falta nesta máquina para o conjunto funcionar.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! ## Por que isto existe
//!
//! Montar uma máquina nova deste conjunto é uma lista de ~10 coisas,
//! diferente em cada sistema, e nenhuma delas dá erro claro quando
//! falta: sem ACL no `/dev/hidraw` o HID++ só não acha dispositivo; sem
//! Gravação de Tela o AVFoundation **trava** em vez de falhar; sem
//! tarefa `/IT` o host do Windows sobe na sessão 0 e captura preto. Em
//! todos esses casos o sintoma é "não funcionou" e a causa está a três
//! comandos de distância.
//!
//! O `status.sh` já respondia isso no terminal, mas só no Linux e só
//! para quem sabe que ele existe. Este módulo é o mesmo diagnóstico com
//! três diferenças que importam: roda nas três máquinas, roda **de
//! dentro** do daemon (então mede a própria sessão, não a do SSH -- ver
//! [[../../.claude/rules/graphical-session]]), e sabe arrumar parte do que
//! encontra.
//!
//! ## A regra de honestidade
//!
//! Um passo que não se consegue medir vira [`Estado::Erro`], **nunca**
//! [`Estado::Ok`]. Já tivemos duas proteções que não protegiam nada
//! (`dispatch.backfill_gate` em outro projeto, o `catch` do langfuse) e
//! o padrão é sempre o mesmo: o silêncio parecia sucesso. Aqui "não sei"
//! é um estado visível, e bloqueia o "pronto" igual a "falta".
//!
//! Depois de arrumar, **mede de novo**. O botão nunca pinta verde por
//! ter rodado o comando; pinta por ter medido o resultado
//! ([[../../.claude/rules/verify-what-arrived]]).
//!
//! ## O que é puro e o que não é
//!
//! Este arquivo é o catálogo e a decisão -- quais passos existem em cada
//! sistema, em que ordem, quem espera por quem, e quando o conjunto está
//! pronto. Nada aqui toca disco, rede ou processo, e por isso os testes
//! rodam no laptop de desenvolvimento cobrindo os **três** catálogos,
//! não só o desta máquina. Foi o que pegou a dependência pendurada no
//! `catalogo(Windows)` antes de ela existir numa máquina.
//!
//! A medição real e os reparos ficam em `setup_linux.rs`,
//! `setup_mac.rs` e `setup_win.rs`.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sistema {
    Linux,
    Mac,
    Windows,
}

impl Sistema {
    pub fn desta_maquina() -> Self {
        if cfg!(target_os = "macos") {
            Sistema::Mac
        } else if cfg!(windows) {
            Sistema::Windows
        } else {
            Sistema::Linux
        }
    }
}

/// Como este passo se arruma -- e quem tem de agir.
///
/// A distinção entre `Sudo` e `Clique` não é cosmética: a primeira a
/// pessoa resolve colando uma linha em qualquer terminal, inclusive por
/// SSH; a segunda **só** funciona na frente da máquina, porque é o TCC
/// do macOS ou o UAC do Windows pedindo consentimento humano. Misturar
/// as duas é prometer automação que não existe.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "tipo", rename_all = "lowercase")]
pub enum Arrumo {
    /// O daemon faz agora, sozinho, e mede de novo.
    Sozinho { rotulo: &'static str },
    /// Precisa de root. Mostramos a linha exata para copiar.
    Sudo { linha: &'static str },
    /// Precisa de um clique humano numa janela do sistema. O daemon
    /// abre a janela certa; quem decide é a pessoa.
    Clique { rotulo: &'static str, onde: &'static str },
    /// Não há automação honesta aqui. A receita está escrita.
    Receita { linha: &'static str },
}

#[derive(Debug, Clone, Serialize)]
pub struct Passo {
    pub id: &'static str,
    pub titulo: &'static str,
    /// Uma linha dizendo o que quebra sem isso.
    ///
    /// Existe porque "ative isto" sem motivo vira clique automático, e
    /// três meses depois nem eu lembro por que o passo está na lista.
    pub porque: &'static str,
    pub arrumo: Arrumo,
    /// Ids que precisam estar `Ok` antes deste fazer sentido.
    pub depende: &'static [&'static str],
    /// `false` = o conjunto funciona sem isso, só perde uma parte.
    pub essencial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Estado {
    Ok,
    Falta,
    /// Medir falhou. Não é "ok" e não é "falta": é "não sei".
    Erro,
    /// Não se aplica a esta máquina (sem GPU, sem receptor).
    Fora,
}

#[derive(Debug, Clone, Serialize)]
pub struct Medida {
    pub id: String,
    pub estado: Estado,
    pub detalhe: String,
}

impl Medida {
    pub fn ok(id: &str, detalhe: impl Into<String>) -> Self {
        Medida { id: id.into(), estado: Estado::Ok, detalhe: detalhe.into() }
    }
    pub fn falta(id: &str, detalhe: impl Into<String>) -> Self {
        Medida { id: id.into(), estado: Estado::Falta, detalhe: detalhe.into() }
    }
    pub fn erro(id: &str, detalhe: impl Into<String>) -> Self {
        Medida { id: id.into(), estado: Estado::Erro, detalhe: detalhe.into() }
    }
    pub fn fora(id: &str, detalhe: impl Into<String>) -> Self {
        Medida { id: id.into(), estado: Estado::Fora, detalhe: detalhe.into() }
    }
    /// Atalho para medição booleana, que é a maioria.
    pub fn se(id: &str, cond: bool, bom: impl Into<String>, ruim: impl Into<String>) -> Self {
        if cond {
            Medida::ok(id, bom)
        } else {
            Medida::falta(id, ruim)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Resumo {
    /// Passos essenciais que se aplicam a esta máquina.
    pub total: usize,
    pub prontos: usize,
    /// Todo essencial aplicável está `Ok`.
    pub pronto: bool,
    /// Primeiro passo em que agir faz diferença agora.
    pub proximo: Option<String>,
    /// Passos que falham mas dependem de outro que falha antes.
    pub esperando: Vec<String>,
}

pub fn estado_de(medidas: &[Medida], id: &str) -> Estado {
    // Ausente = Erro, não Ok. Um passo que ninguém mediu não passou:
    // esquecer de medir é exatamente o jeito de um "pronto" mentir.
    medidas.iter().find(|m| m.id == id).map(|m| m.estado).unwrap_or(Estado::Erro)
}

/// Qual dependência deste passo ainda não está de pé.
///
/// Devolve a **primeira** -- a ordem do catálogo é a ordem de resolução,
/// então a primeira é a que a pessoa deve atacar.
pub fn bloqueado_por(p: &Passo, medidas: &[Medida]) -> Option<String> {
    p.depende
        .iter()
        .find(|d| !matches!(estado_de(medidas, d), Estado::Ok | Estado::Fora))
        .map(|d| (*d).to_string())
}

pub fn resumo(passos: &[Passo], medidas: &[Medida]) -> Resumo {
    let mut total = 0;
    let mut prontos = 0;
    let mut pronto = true;
    let mut proximo = None;
    let mut esperando = Vec::new();

    for p in passos {
        let e = estado_de(medidas, p.id);
        if e == Estado::Fora {
            // Não se aplica: não entra na conta nem atrasa ninguém.
            // Contar como pendência faria o painel acusar falta de GPU
            // numa máquina que nunca vai ter GPU.
            continue;
        }
        if p.essencial {
            total += 1;
            if e == Estado::Ok {
                prontos += 1;
            } else {
                pronto = false;
            }
        }
        if e != Estado::Ok {
            match bloqueado_por(p, medidas) {
                Some(_) => esperando.push(p.id.to_string()),
                None if proximo.is_none() => proximo = Some(p.id.to_string()),
                None => {}
            }
        }
    }

    Resumo { total, prontos, pronto, proximo, esperando }
}

// ---------------------------------------------------------------- catálogos

/// Passos desta máquina, na ordem de resolução.
pub fn catalogo_local() -> Vec<Passo> {
    catalogo(Sistema::desta_maquina())
}

pub fn catalogo(s: Sistema) -> Vec<Passo> {
    match s {
        Sistema::Linux => linux(),
        Sistema::Mac => mac(),
        Sistema::Windows => windows(),
    }
}

fn linux() -> Vec<Passo> {
    vec![
        Passo {
            id: "binarios",
            titulo: "Binários instalados",
            porque: "o daemon, o host de tela e o KVM vivem em ~/.local/bin. \
                     Faltou o logistream-kvm uma vez e o botão da bandeja \
                     falhava com \"não consegui executar\" -- compilado, \
                     nunca instalado.",
            arrumo: Arrumo::Receita { linha: "bash install.sh   # ou ./scripts/deploy.sh" },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "config",
            titulo: "Config desta máquina",
            porque: "cada máquina tem a própria config e nada sincroniza. \
                     Sem a entrada desta máquina marcada como local, o \
                     painel não sabe quem é \"aqui\".",
            arrumo: Arrumo::Sozinho { rotulo: "Escrever a config padrão" },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "daemon",
            titulo: "Serviço de usuário ativo",
            porque: "o logid.service é quem sobe o painel no logon. Rodando \
                     na mão funciona até a primeira vez que você reinicia.",
            arrumo: Arrumo::Sozinho { rotulo: "Habilitar e iniciar" },
            depende: &["binarios"],
            essencial: true,
        },
        Passo {
            id: "receptor",
            titulo: "Acesso ao receptor (HID++)",
            porque: "é por /dev/hidraw que trocamos o canal Easy-Switch do \
                     teclado e do mouse. Sem acesso o painel simplesmente \
                     não acha dispositivo -- não diz \"sem permissão\".",
            arrumo: Arrumo::Sudo {
                linha: "sudo install -m644 ~/dev/logi/logid/udev/70-logid-hidraw.rules /etc/udev/rules.d/\n\
                        sudo udevadm control --reload\n\
                        sudo udevadm trigger --subsystem-match=hidraw --action=add",
            },
            depende: &[],
            essencial: false,
        },
        Passo {
            id: "input-kvm",
            titulo: "Acesso ao teclado e mouse (KVM por LAN)",
            porque: "o KVM lê /dev/input/event* e chama EVIOCGRAB. A regra usa \
                     TAG+=\"uaccess\" restrito ao vendor 046d, e NÃO o grupo \
                     input: o grupo daria a qualquer processo da conta acesso \
                     permanente a todos os dispositivos de entrada -- um \
                     keylogger passivo de graça.",
            arrumo: Arrumo::Sudo {
                linha: "sudo install -m644 ~/dev/logi/logid/udev/70-logid-input.rules /etc/udev/rules.d/\n\
                        sudo udevadm control --reload\n\
                        sudo udevadm trigger --subsystem-match=input",
            },
            depende: &[],
            essencial: false,
        },
        Passo {
            id: "wayland",
            titulo: "Sessão Wayland",
            porque: "a detecção de borda usa a API privada do Mutter \
                     (org.gnome.Mutter.InputCapture), e o clipboard usa a \
                     RemoteDesktop do mesmo Mutter. No X11 nenhum dos dois \
                     foi validado.",
            arrumo: Arrumo::Receita { linha: "escolha \"GNOME\" (não \"GNOME on Xorg\") na tela de login" },
            depende: &[],
            essencial: false,
        },
        Passo {
            id: "clipboard",
            titulo: "Clipboard entre as máquinas",
            porque: "o Mutter precisa aceitar a sessão de RemoteDesktop para \
                     observarmos a área de transferência. Sem isso a cópia \
                     não atravessa.",
            arrumo: Arrumo::Sozinho { rotulo: "Testar agora" },
            depende: &["daemon"],
            essencial: true,
        },
        Passo {
            id: "pares",
            titulo: "SSH alcança as outras máquinas",
            porque: "SSH é o transporte de tudo que cruza máquina -- troca de \
                     tela, clipboard, arquivos. Sem chave autorizada o painel \
                     mostra as outras como mortas.",
            arrumo: Arrumo::Receita { linha: "ssh-copy-id <alias>   # alias é o campo `ssh` de cada host na config" },
            depende: &["config"],
            essencial: true,
        },
        Passo {
            id: "nvenc",
            titulo: "Codificador de vídeo por hardware",
            porque: "esta máquina só pode ser ASSISTIDA se tiver NVENC. Sem \
                     ele o painel cai no caminho de software, que é mais \
                     lento e esquenta.",
            arrumo: Arrumo::Receita { linha: "driver NVIDIA proprietário instalado; confira com nvidia-smi" },
            depende: &["binarios"],
            essencial: false,
        },
        Passo {
            id: "bandeja",
            titulo: "Ícone na bandeja",
            porque: "é o caminho mais curto para trazer o teclado de volta \
                     quando ele está em outra máquina. Sem ele você depende \
                     de lembrar os dois Ctrl.",
            arrumo: Arrumo::Sozinho { rotulo: "Lançar agora" },
            depende: &["daemon"],
            essencial: false,
        },
        Passo {
            id: "ota",
            titulo: "Atualização automática (OTA)",
            porque: "esta máquina se atualiza sozinha pelas Releases do \
                     GitHub -- é o que mantém o Mac em dia quando ele \
                     está longe de casa e ninguém pode rodar o deploy. \
                     Sem o relógio, o build velho fica.",
            arrumo: Arrumo::Sozinho { rotulo: "Instalar o relógio e conferir agora" },
            depende: &["binarios"],
            essencial: false,
        },
    ]
}

fn mac() -> Vec<Passo> {
    vec![
        Passo {
            id: "binarios",
            titulo: "Binários instalados",
            porque: "o Mac é cliente E host de tela. Deixar o host para trás \
                     já aconteceu: a máquina assistia as outras e nunca era \
                     assistida, e o painel caía no VNC sem dizer por quê.",
            arrumo: Arrumo::Receita { linha: "./scripts/deploy.sh   # compila no próprio Mac, sem cross-compile" },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "config",
            titulo: "Config desta máquina",
            porque: "cada máquina tem a própria config e nada sincroniza.",
            arrumo: Arrumo::Sozinho { rotulo: "Escrever a config padrão" },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "daemon",
            titulo: "LaunchAgent do daemon",
            porque: "o logid tem de nascer na sessão gráfica. Lançado por SSH \
                     ele não enxerga nem o chaveiro nem as permissões.",
            arrumo: Arrumo::Sozinho { rotulo: "Instalar e carregar" },
            depende: &["binarios"],
            essencial: true,
        },
        Passo {
            id: "ffmpeg",
            titulo: "ffmpeg que os binários pedem",
            porque: "o Mac é a única máquina que liga no ffmpeg -- captura \
                     por avfoundation, codificação por h264_videotoolbox -- \
                     e o soname entra NO BINÁRIO: compilado contra o major \
                     9 ele não carrega com o major 7 instalado. Sem a \
                     formula certa a tela não sobe e o OTA recusa o update \
                     inteiro: está certo, mas chega como recusa de hora em \
                     hora, e não como uma linha aqui.",
            arrumo: Arrumo::Receita { linha: "brew install ffmpeg@7" },
            depende: &["binarios"],
            essencial: true,
        },
        Passo {
            id: "identidade",
            titulo: "Identidade local de assinatura",
            porque: "sem ela o binário é ad-hoc, a amarração do TCC é pelo \
                     cdhash e a permissão de tela MORRE a cada compilação. \
                     Medido: conceder, rodar um deploy, e o host voltar a \
                     travar sem erro nenhum no log.",
            arrumo: Arrumo::Receita {
                linha: "bash ~/.local/bin/criar-identidade.sh   # no Terminal do Mac: por SSH o chaveiro está trancado",
            },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "assinatura",
            titulo: "Host de tela assinado",
            porque: "é o que faz a permissão sobreviver ao próximo deploy. \
                     Conceder a permissão ANTES de assinar desperdiça a \
                     concessão -- e só o dono da máquina pode dar de volta, \
                     à mão.",
            arrumo: Arrumo::Sozinho { rotulo: "Assinar agora" },
            depende: &["binarios", "identidade"],
            essencial: true,
        },
        Passo {
            id: "gravacao-de-tela",
            titulo: "Permissão de Gravação de Tela",
            porque: "sem ela o AVFoundation TRAVA em vez de falhar: o log \
                     para em \"abrindo a tela 1\" e a última linha é uma \
                     mensagem do ffmpeg sobre formato de pixel, que não tem \
                     nada a ver com o problema. Cuidado: a PRIMEIRA \
                     abertura depois de trocar o binário levou ~30 s, com o \
                     log parado nessa mesma frase (as seguintes, ~3 s). \
                     Espere 30 s antes de culpar o TCC.",
            arrumo: Arrumo::Clique {
                rotulo: "Pedir a permissão",
                onde: "Ajustes > Privacidade e Segurança > Gravação de Tela > logistream-host",
            },
            depende: &["assinatura"],
            essencial: true,
        },
        Passo {
            id: "acessibilidade",
            titulo: "Permissão de Acessibilidade",
            porque: "é o que permite injetar teclado e mouse. Sem ela a tela \
                     aparece e o ponteiro não anda -- mesmo sintoma de \"não \
                     funcionou\", causa diferente.",
            arrumo: Arrumo::Clique {
                rotulo: "Pedir a permissão",
                onde: "Ajustes > Privacidade e Segurança > Acessibilidade > logistream-host",
            },
            depende: &["assinatura"],
            essencial: true,
        },
        Passo {
            id: "launchagent",
            titulo: "LaunchAgent do host de tela",
            porque: "o host sobe por launchctl na sessão gráfica, nunca por \
                     SSH. RunAtLoad false de propósito: capturar sem ninguém \
                     assistindo gastaria bateria à toa.",
            arrumo: Arrumo::Sozinho { rotulo: "Instalar e carregar" },
            depende: &["binarios"],
            essencial: true,
        },
        Passo {
            id: "clipboard",
            titulo: "Clipboard entre as máquinas",
            porque: "a leitura do clipboard do Mac passa por JXA, e isso só \
                     roda na sessão gráfica.",
            arrumo: Arrumo::Sozinho { rotulo: "Testar agora" },
            depende: &["daemon"],
            essencial: true,
        },
        Passo {
            id: "login-remoto",
            titulo: "Login Remoto (SSH de entrada)",
            porque: "é o que deixa o deploy e o painel alcançarem este Mac. \
                     Sem isso ele só fala, nunca escuta.",
            arrumo: Arrumo::Sudo { linha: "sudo systemsetup -f -setremotelogin on" },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "tailscale",
            titulo: "Tailscale (fora da LAN)",
            porque: "é o que alcança as outras máquinas fora de casa. Vai pelo \
                     brew, com serviço próprio, e nunca pelo app: a extensão \
                     de rede do app, sem login, derrubou a internet do Mac.",
            arrumo: Arrumo::Clique {
                rotulo: "Instalar pelo brew",
                onde: "o Terminal que vai abrir: digite a senha e abra o link de login",
            },
            depende: &[],
            essencial: false,
        },
        Passo {
            id: "pares",
            titulo: "SSH alcança as outras máquinas",
            porque: "SSH é o transporte de tudo que cruza máquina.",
            arrumo: Arrumo::Receita { linha: "ssh-copy-id <alias>" },
            depende: &["config"],
            essencial: true,
        },
        Passo {
            id: "ota",
            titulo: "Atualização automática (OTA)",
            porque: "é o passo que faz este Mac se manter em dia longe de \
                     casa. O agente baixa o build publicado, **assina com a \
                     identidade local** e só então troca -- é isso que \
                     preserva a Gravação de Tela, que um binário ad-hoc da \
                     CI perderia.",
            arrumo: Arrumo::Sozinho { rotulo: "Instalar o relógio e conferir agora" },
            depende: &["binarios", "identidade"],
            essencial: false,
        },
    ]
}

fn windows() -> Vec<Passo> {
    vec![
        Passo {
            id: "binarios",
            titulo: "Binários instalados",
            porque: "o daemon, o agente, o host e o visualizador vivem em \
                     %LOCALAPPDATA%\\logid. O deploy assina cada um -- sem \
                     assinatura o Defender come o executável sem avisar.",
            arrumo: Arrumo::Receita { linha: "./scripts/deploy.sh   # do Ubuntu: cross-compile, assina e troca" },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "config",
            titulo: "Config desta máquina",
            porque: "cada máquina tem a própria config e nada sincroniza.",
            arrumo: Arrumo::Sozinho { rotulo: "Escrever a config padrão" },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "daemon",
            titulo: "Tarefa de logon do daemon",
            porque: "no Windows o logid sobe por tarefa agendada com /IT, não \
                     por serviço: serviço nasce na sessão 0 e de lá não se vê \
                     teclado, mouse nem área de transferência.",
            arrumo: Arrumo::Sozinho { rotulo: "Criar a tarefa" },
            depende: &["binarios"],
            essencial: true,
        },
        Passo {
            id: "tarefa-tela",
            titulo: "Tarefa interativa do host de tela",
            porque: "mesma fronteira: sem /IT o DXGI devolve E_ACCESSDENIED \
                     ou captura preto, e o sintoma não diz nada sobre sessão.",
            arrumo: Arrumo::Sozinho { rotulo: "Criar a tarefa" },
            depende: &["binarios"],
            essencial: true,
        },
        Passo {
            id: "clipboard",
            titulo: "Clipboard entre as máquinas",
            porque: "ler a área de transferência exige estar na sessão \
                     interativa. Pelo SSH se lê a sessão 0, que está sempre \
                     vazia -- e \"vazio\" parece \"nada copiado\".",
            arrumo: Arrumo::Sozinho { rotulo: "Testar agora" },
            depende: &["daemon"],
            essencial: true,
        },
        Passo {
            id: "sshd",
            titulo: "Servidor OpenSSH",
            porque: "é por onde o Ubuntu manda deploy, clipboard e arquivos. \
                     Sem ele esta máquina some do painel.",
            arrumo: Arrumo::Sudo {
                linha: "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0; Set-Service sshd -StartupType Automatic; Start-Service sshd",
            },
            depende: &[],
            essencial: true,
        },
        Passo {
            id: "chave-admin",
            titulo: "Chave autorizada",
            porque: "o OpenSSH do Windows lê a chave de um administrador em \
                     ProgramData\\ssh\\administrators_authorized_keys, NÃO no \
                     ~/.ssh do usuário. Põr no lugar errado falha com \
                     \"Permission denied\" e nenhuma pista.",
            arrumo: Arrumo::Receita {
                linha: "ssh-copy-id não existe aqui: ver .claude/rules/remote-powershell.md",
            },
            depende: &["sshd"],
            essencial: true,
        },
        Passo {
            id: "pares",
            titulo: "SSH alcança as outras máquinas",
            porque: "SSH é o transporte de tudo que cruza máquina.",
            arrumo: Arrumo::Receita { linha: "ssh-keygen e copiar a pública para o outro lado" },
            depende: &["config"],
            essencial: true,
        },
        Passo {
            id: "tela-virtual",
            titulo: "Tela sem monitor ligado",
            porque: "sem monitor o Windows não tem o que capturar: o visualizador \
                     conecta, o clique passa e nenhum quadro chega. Uma tela \
                     virtual mantém o acesso com a máquina sozinha.",
            arrumo: Arrumo::Clique {
                rotulo: "Instalar a tela virtual",
                onde: "o pedido de administrador do Windows",
            },
            depende: &[],
            essencial: false,
        },
        Passo {
            id: "protetor-de-tela",
            titulo: "Protetor de tela e bloqueio",
            porque: "com a sessão bloqueada o DXGI devolve E_ACCESSDENIED e a \
                     tela remota morre sozinha depois de alguns minutos. A \
                     escolha é sua: desligar o bloqueio, ou saber que a tela \
                     cai quando ele entra.",
            arrumo: Arrumo::Receita {
                linha: "powercfg /setacvalueindex SCHEME_CURRENT SUB_NONE CONSOLELOCK 0",
            },
            depende: &[],
            essencial: false,
        },
        Passo {
            id: "ota",
            titulo: "Atualização automática (OTA)",
            porque: "aqui o OTA **só confere e relata**: a chave do \
                     Authenticode não sai do Ubuntu, e `.exe` sem assinatura \
                     válida é o que o Defender apaga na escrita. Serve para \
                     o painel dizer quando esta máquina ficou atrás -- quem \
                     troca é o deploy.",
            arrumo: Arrumo::Sozinho { rotulo: "Instalar a tarefa e conferir agora" },
            depende: &["binarios"],
            essencial: false,
        },
    ]
}

#[cfg(test)]
mod testes {
    use super::*;

    const TRES: [Sistema; 3] = [Sistema::Linux, Sistema::Mac, Sistema::Windows];

    fn m(id: &str, e: Estado) -> Medida {
        Medida { id: id.into(), estado: e, detalhe: String::new() }
    }

    #[test]
    fn erro_nao_conta_como_ok() {
        // O ponto da regra de honestidade: "não sei medir" tem de pesar
        // igual a "falta". Se pesasse menos, o passo mais difícil de
        // medir seria o mais fácil de passar.
        let passos = linux();
        let medidas: Vec<_> = passos.iter().map(|p| m(p.id, Estado::Erro)).collect();
        let r = resumo(&passos, &medidas);
        assert!(!r.pronto);
        assert_eq!(r.prontos, 0);
    }

    #[test]
    fn passo_sem_medida_nao_passa() {
        // Esquecer de medir um passo é o jeito mais fácil de um "pronto"
        // mentir -- e o mais provável, porque o catálogo e a medição
        // moram em arquivos diferentes.
        let passos = linux();
        let medidas: Vec<_> = passos
            .iter()
            .filter(|p| p.id != "clipboard")
            .map(|p| m(p.id, Estado::Ok))
            .collect();
        let r = resumo(&passos, &medidas);
        assert!(!r.pronto);
        assert_eq!(estado_de(&medidas, "clipboard"), Estado::Erro);
    }

    #[test]
    fn fora_nao_bloqueia_nem_conta() {
        let passos = linux();
        let total_antes = {
            let todas: Vec<_> = passos.iter().map(|p| m(p.id, Estado::Ok)).collect();
            resumo(&passos, &todas).total
        };
        let medidas: Vec<_> = passos
            .iter()
            .map(|p| m(p.id, if p.id == "clipboard" { Estado::Fora } else { Estado::Ok }))
            .collect();
        let r = resumo(&passos, &medidas);
        assert!(r.pronto, "um passo fora de escopo não impede o pronto");
        assert_eq!(r.total, total_antes - 1, "e também não entra na conta");
        assert_eq!(r.proximo, None);
    }

    #[test]
    fn nao_essencial_nao_impede_pronto() {
        let passos = linux();
        let medidas: Vec<_> = passos
            .iter()
            .map(|p| m(p.id, if p.essencial { Estado::Ok } else { Estado::Falta }))
            .collect();
        let r = resumo(&passos, &medidas);
        assert!(r.pronto);
        // Mas ainda aparece como próxima ação: não impedir não é esconder.
        assert!(r.proximo.is_some());
    }

    #[test]
    fn proximo_pula_quem_espera_dependencia() {
        // `daemon` depende de `binarios`. Com os dois faltando, a ação
        // oferecida tem de ser instalar os binários -- mandar habilitar
        // um serviço cujo executável não existe não arruma nada e ensina
        // a pessoa a desconfiar do wizard.
        let passos = linux();
        let medidas: Vec<_> = passos
            .iter()
            .map(|p| {
                let falta = p.id == "daemon" || p.id == "binarios";
                m(p.id, if falta { Estado::Falta } else { Estado::Ok })
            })
            .collect();
        let r = resumo(&passos, &medidas);
        assert_eq!(r.proximo.as_deref(), Some("binarios"));
        assert!(r.esperando.contains(&"daemon".to_string()));
    }

    #[test]
    fn dependencia_fora_de_escopo_libera_quem_espera() {
        // `Fora` não é falha: se a dependência não se aplica a esta
        // máquina, quem espera por ela tem de poder andar.
        let p = Passo {
            id: "x",
            titulo: "",
            porque: "",
            arrumo: Arrumo::Sozinho { rotulo: "" },
            depende: &["y"],
            essencial: true,
        };
        assert_eq!(bloqueado_por(&p, &[m("y", Estado::Fora)]), None);
        assert_eq!(bloqueado_por(&p, &[m("y", Estado::Falta)]).as_deref(), Some("y"));
    }

    #[test]
    fn tudo_ok_fica_pronto() {
        for s in TRES {
            let passos = catalogo(s);
            let medidas: Vec<_> = passos.iter().map(|p| m(p.id, Estado::Ok)).collect();
            let r = resumo(&passos, &medidas);
            assert!(r.pronto, "{s:?}");
            assert_eq!(r.proximo, None, "{s:?}");
            assert_eq!(r.prontos, r.total, "{s:?}");
            assert!(r.total > 0, "{s:?}");
        }
    }

    #[test]
    fn ids_unicos_em_cada_catalogo() {
        for s in TRES {
            let passos = catalogo(s);
            let mut ids: Vec<_> = passos.iter().map(|p| p.id).collect();
            let antes = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), antes, "id repetido em {s:?}");
        }
    }

    #[test]
    fn dependencia_existe_e_vem_antes() {
        // Dependência pendurada deixaria o passo esperando para sempre,
        // e o wizard nunca ofereceria ação nenhuma -- uma tela de "falta
        // isso" sem botão. Dependência que aponta para FRENTE mostraria
        // "esperando" acima da coisa esperada, que é como se lê de
        // baixo para cima.
        for s in TRES {
            let passos = catalogo(s);
            for (i, p) in passos.iter().enumerate() {
                for d in p.depende {
                    let pos = passos.iter().position(|q| q.id == *d);
                    let pos = pos.unwrap_or_else(|| panic!("{s:?}: {} depende de {d}, que não existe", p.id));
                    assert!(pos < i, "{s:?}: {} depende de {d}, que vem depois", p.id);
                }
            }
        }
    }

    #[test]
    fn todo_passo_diz_por_que() {
        // Um passo sem motivo vira clique automático. Barato de testar,
        // e o teste é o que impede a pressa de encostar um `porque: ""`.
        for s in TRES {
            for p in catalogo(s) {
                assert!(p.porque.len() > 40, "{s:?}: {} sem motivo escrito", p.id);
                assert!(!p.titulo.is_empty(), "{s:?}: {} sem título", p.id);
            }
        }
    }
}

// ------------------------------------------------------- despacho por sistema
//
// Só aqui o módulo puro toca no mundo, e só por delegação: a medição
// real vive no arquivo da plataforma. A alternativa -- `cfg` espalhado
// pelo catálogo -- faria os testes acima cobrirem apenas o sistema que
// está compilando, que é justamente o que eles existem para evitar.

/// Mede os passos desta máquina, na ordem do catálogo.
pub fn medir(cfg: &crate::config::Config) -> Vec<Medida> {
    #[cfg(target_os = "linux")]
    return crate::setup_linux::medir(cfg);
    #[cfg(target_os = "macos")]
    return crate::setup_mac::medir(cfg);
    #[cfg(windows)]
    return crate::setup_win::medir(cfg);
}

/// Tenta arrumar um passo e devolve **o que mediu depois**.
pub fn arrumar(id: &str, cfg: &crate::config::Config) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    return crate::setup_linux::arrumar(id, cfg);
    #[cfg(target_os = "macos")]
    return crate::setup_mac::arrumar(id, cfg);
    #[cfg(windows)]
    return crate::setup_win::arrumar(id, cfg);
}
