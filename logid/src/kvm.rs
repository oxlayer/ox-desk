//! KVM por LAN: manda teclado e mouse para outra maquina pela rede.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! A alternativa ao HID++ 0x1814. Aquele manda o PROPRIO dispositivo
//! trocar de canal Easy-Switch, o que exige um receptor pareado na
//! maquina de destino. Com um dongle so -- que foi onde paramos depois
//! que dois dongles perto se provaram instaveis -- apertar "mandar para
//! windows-host" fazia o teclado procurar um receptor que nao existe la,
//! nao achar, e voltar. Era o comportamento correto do protocolo
//! resolvendo o problema errado.
//!
//! Aqui o dispositivo nao sai do lugar: capturamos localmente
//! (`EVIOCGRAB`) e injetamos na outra ponta pelo mesmo stream de input
//! que a tela remota ja usa.
//!
//! ## Um processo filho, nao uma thread
//!
//! O `logistream-kvm` roda como processo separado, e isso e desenho,
//! nao preguica: **matar o processo solta o grab**, porque o kernel
//! solta quando o fd fecha. Se o KVM fosse uma thread do `logid` e ela
//! travasse, o teclado ficaria agarrado e a unica saida seria matar o
//! agente inteiro -- que e a coisa que voce usa para consertar as
//! outras. Processo filho faz da desistencia uma operacao de uma linha.

use std::process::{Child, Stdio};
use std::sync::Mutex;

use crate::config::Config;

/// Quem esta segurando o teclado: um filho nosso, ou um processo que
/// sobreviveu a um reinicio do daemon.
enum Processo {
    /// Nascido nesta instancia do `logid`.
    Nosso(Child),
    /// Encontrado vivo na partida e adotado. So temos o pid.
    Adotado(u32),
}

/// O processo vivo e para quem ele esta mandando.
static ATUAL: Mutex<Option<(String, Processo)>> = Mutex::new(None);

/// O processo ainda existe?
fn vivo(p: &mut Processo) -> bool {
    match p {
        Processo::Nosso(c) => !matches!(c.try_wait(), Ok(Some(_))),
        // `/proc/<pid>` e a pergunta mais barata que existe, e nao
        // depende de o processo ser nosso filho -- `waitpid` dependeria.
        //
        // So existe adocao no Linux, porque so la existe captura. Nos
        // outros este braco e inalcancavel.
        #[cfg(target_os = "linux")]
        Processo::Adotado(pid) => std::path::Path::new(&format!("/proc/{pid}")).exists(),
        #[cfg(not(target_os = "linux"))]
        Processo::Adotado(_) => false,
    }
}

/// Para quem o input esta indo agora, ou `None` se esta aqui.
pub fn alvo() -> Option<String> {
    let mut g = ATUAL.lock().ok()?;
    // Colher o filho antes de responder: se ele morreu (socorro pelos
    // dois Ctrl, alvo caiu, rede sumiu), o input ja voltou para esta
    // maquina e dizer o contrario deixaria a bandeja mentindo.
    if let Some((_, p)) = g.as_mut() {
        if !vivo(p) {
            *g = None;
        }
    }
    g.as_ref().map(|(n, _)| n.clone())
}

/// Traz o input de volta para esta maquina. Idempotente.
pub fn soltar() -> Result<(), String> {
    let mut g = ATUAL.lock().map_err(|_| "estado do KVM travado")?;
    if let Some((nome, p)) = g.take() {
        match p {
            Processo::Nosso(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            // Nao e nosso filho: `kill` do std nao alcanca, entao vai
            // por sinal. SIGTERM e nao SIGKILL de proposito -- o KVM
            // solta as teclas presas do outro lado antes de sair, e
            // SIGKILL pularia isso, deixando Shift travado na outra
            // maquina.
            #[cfg(target_os = "linux")]
            Processo::Adotado(pid) => unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            },
            #[cfg(not(target_os = "linux"))]
            Processo::Adotado(_) => {}
        }
        say!("kvm: input de volta (estava em {nome})");
    }
    Ok(())
}

/// Manda o input para `host`. Solta o alvo anterior, se houver.
///
/// A ordem importa: soltamos o anterior ANTES de tentar o novo. Dois
/// `EVIOCGRAB` no mesmo dispositivo nao coexistem, e o segundo falharia
/// com "Device or resource busy" -- deixando o input preso no destino
/// velho e a mensagem de erro falando de outra coisa.
pub fn mandar(cfg: &Config, host: &str) -> Result<String, String> {
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' nao esta na config"))?;
    // Mandar para a maquina local = trazer de volta. Mesmo teste de
    // "e aqui?" que o focus.rs usa, e nao so a flag `local`: ela e
    // opcional na config e nem todo host a declara, entao confiar so
    // nela faria "Trazer para ubuntu-host" tentar abrir um KVM
    // apontando para nos mesmos.
    let aqui = h.local.unwrap_or(false)
        || matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1");
    if aqui {
        return soltar().map(|()| "input local".to_string());
    }
    let sc = h
        .screen
        .as_ref()
        .ok_or_else(|| format!("'{host}' nao tem `[hosts.screen]`: sem porta para mandar input"))?;

    // O macOS injeta input desde 04/11 (`input_mac.rs`, CGEvent). A
    // recusa que existia aqui saiu junto.
    //
    // O que NAO da para verificar daqui e se o host de la tem permissao
    // de Acessibilidade: sem ela o `CGEventPost` aceita o evento e o
    // descarta em silencio. Quem avisa e o proprio host, na partida, no
    // log dele -- e e la que se procura quando o KVM "nao faz nada".

    soltar()?;

    // Esta maquina sabe capturar?
    //
    // So o Linux tem captura (`EVIOCGRAB`). Num Windows ou Mac, clicar
    // "Mandar para ubuntu-host" nao pode significar "vire origem" --
    // significa "quero o input LA", e quem tem de agir e quem esta
    // segurando o teclado agora: a origem.
    //
    // Sem isto o clique falhava em silencio e o unico caminho de volta
    // era o socorro dos dois Ctrl. O usuario relatou exatamente assim:
    // "tentei clicar no tray do windows para mandar para ubuntu-host
    // e nao voltou, so funcionou apertando dois ctrl".
    if !cfg!(target_os = "linux") {
        return pedir_para_soltar(h).map(|()| format!("pedi a {host} que retomasse o input"));
    }

    // Levantar o host da outra ponta primeiro. E ele quem recebe o
    // stream de input; sem ele de pe o `logistream-kvm` recusa conectar
    // -- e recusar e o comportamento certo, porque agarrar o teclado
    // sem destino deixaria a pessoa sem teclado E sem para onde ir.
    crate::screen::acordar_host(h, sc.port)
        .map_err(|e| format!("nao consegui levantar o host de {host}: {e}"))?;

    // `porta + 1`: a porta de INPUT do host, nao a de video.
    //
    // O host serve um cliente de video por vez. Enquanto o KVM
    // conectava na porta de video, ele tomava a vaga do visualizador --
    // o input chegava e era aplicado, mas a tela que a pessoa estava
    // olhando congelava, e a conclusao obvia era "nao funcionou".
    // Portas separadas tambem evitam a maquina alvo codificar video
    // para um cliente que nunca desenha nada.
    let destino = format!("{}:{}", h.addr, sc.port + 1);

    // Saida do filho num arquivo, nao em /dev/null.
    //
    // A primeira versao jogava fora stdout e stderr, como o lancamento
    // do cliente de tela faz. Mas aquele abre uma JANELA: se ele morre,
    // a pessoa ve. Este nao abre nada -- o unico sinal de que falhou
    // seria o teclado continuar aqui, sem uma linha dizendo por que.
    let registro = crate::sys::state_dir().join("logistream-kvm.log");
    let saida = std::fs::File::create(&registro)
        .map_err(|e| format!("nao consegui abrir {}: {e}", registro.display()))?;
    let erro = saida
        .try_clone()
        .map_err(|e| format!("nao consegui duplicar o descritor: {e}"))?;

    // Alvo Linux aplica a propria repeticao de tecla (o Mutter repete
    // teclas injetadas), entao mandar a do kernel junto dobraria. No
    // Windows e o contrario: `SendInput` nao repete nada, e sem a do
    // kernel segurar backspace apaga um caractere so.
    let mut cmd = crate::sys::child("logistream-kvm");
    cmd.arg(&destino);
    if h.os.as_deref() == Some("linux") {
        cmd.arg("--sem-repeticao");
    }
    let mut filho = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(saida))
        .stderr(Stdio::from(erro))
        .spawn()
        .map_err(|e| format!("nao consegui executar logistream-kvm: {e}"))?;

    // Confirmar que ele SOBREVIVEU, nao so que nasceu.
    //
    // `spawn` devolve Ok assim que o processo existe -- antes de ele
    // conectar e antes de agarrar. Reportar sucesso ali faria a bandeja
    // dizer "mandado para windows-host" enquanto o filho ja morreu porque
    // o alvo recusou a conexao ou faltou ACL no /dev/input. O estado se
    // corrigiria sozinho no proximo `alvo()`, mas o usuario ja teria
    // lido a mentira e estaria esperando o teclado mudar de maquina.
    //
    // 500 ms cobrem conectar + agarrar numa LAN; o caminho feliz nao
    // sente, e o caminho triste ganha a mensagem de erro de verdade.
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Ok(Some(status)) = filho.try_wait() {
        let motivo = std::fs::read_to_string(&registro)
            .unwrap_or_default()
            .lines()
            .last()
            .unwrap_or("sem detalhe no log")
            .trim()
            .to_string();
        return Err(format!("o KVM saiu ({status}) antes de agarrar: {motivo}"));
    }

    match ATUAL.lock() {
        Ok(mut g) => *g = Some((host.to_string(), Processo::Nosso(filho))),
        Err(_) => return Err("estado do KVM travado".into()),
    }
    say!("kvm: input indo para {host} ({destino})");
    Ok(destino)
}

/// Pede ao `logid` de outra maquina que solte o KVM dela.
///
/// Por SSH e `logid kvm-soltar`, pelo mesmo caminho e pelos mesmos
/// motivos do clipboard: a API escuta so em 127.0.0.1, e SSH ja e o
/// canal confiavel entre as maquinas.
///
/// Nao e erro se a origem nao estiver mandando nada -- soltar o que
/// nao existe e no-op, e o clique continua querendo dizer a mesma
/// coisa.
fn pedir_para_soltar(h: &crate::config::HostCfg) -> Result<(), String> {
    let destino = h.ssh.clone().ok_or_else(|| {
        format!("'{}' nao tem `ssh` na config: nao da para pedir nada a ele", h.name)
    })?;
    let saida = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", &destino])
        .arg(comando_remoto_soltar(h))
        .output()
        .map_err(|e| format!("ssh para {}: {e}", h.name))?;
    if saida.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} recusou soltar: {}",
            h.name,
            String::from_utf8_lossy(&saida.stderr).trim()
        ))
    }
}

/// Mesma regra de caminho do clipboard: por SO, nao pelo PATH.
fn comando_remoto_soltar(h: &crate::config::HostCfg) -> String {
    if h.os.as_deref() == Some("windows") {
        r#"& "$env:LOCALAPPDATA\logid\logid.exe" kvm-soltar"#.into()
    } else {
        "~/.local/bin/logid kvm-soltar".into()
    }
}

/// `logid kvm-soltar`: manda o daemon local devolver o input.
///
/// Passa pela API local pelo mesmo motivo do `clip-set`: o estado do
/// KVM vive no daemon, e um CLI separado nao alcanca o processo filho
/// nem o `ATUAL`.
pub fn cli_soltar(porta: u16) -> std::process::ExitCode {
    use std::io::{Read, Write};
    let pedido = "POST /api/kvm/soltar HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", porta)) else {
        eprintln!("kvm-soltar: o logid local nao respondeu na porta {porta}");
        return std::process::ExitCode::FAILURE;
    };
    if s.write_all(pedido.as_bytes()).is_err() {
        eprintln!("kvm-soltar: falhei ao escrever o pedido");
        return std::process::ExitCode::FAILURE;
    }
    let mut r = String::new();
    let _ = s.read_to_string(&mut r);
    if r.starts_with("HTTP/1.1 200") {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("kvm-soltar: {}", r.lines().next().unwrap_or("sem resposta"));
        std::process::ExitCode::FAILURE
    }
}

/// Adota um `logistream-kvm` que sobreviveu ao reinicio do daemon.
///
/// **Antes isto matava.** O efeito pratico era brutal: o usuario estava
/// trabalhando na outra maquina, um deploy reiniciava o `logid`, e o
/// teclado voltava sozinho para o Ubuntu no meio de uma frase. Ele
/// relatou exatamente assim -- "do nada voltou para ca teclado e
/// mouse, nao estamos fazendo por lan agora?".
///
/// Matar era defensavel quando o KVM era novo: processo agarrando
/// teclado sem dono assusta. Mas o dono volta em segundos, e a
/// alternativa e arrancar o input de alguem que esta usando. Adotar e
/// o certo.
///
/// O alvo vem da propria linha de comando do processo (`<host>:<porta>`),
/// e a porta-1 mapeia de volta para o host da config -- o KVM conecta
/// em `porta + 1`.
///
/// Exige `KillMode=process` no systemd: sem isso o `restart` derruba o
/// cgroup inteiro e nao sobra ninguem para adotar.
#[cfg(target_os = "linux")]
pub fn adotar(cfg: &Config) {
    let saida = std::process::Command::new("pgrep")
        .args(["-a", "-x", "logistream-kvm"])
        .output();
    let Ok(o) = saida else { return };
    let texto = String::from_utf8_lossy(&o.stdout);
    let Some(linha) = texto.lines().next() else { return };

    // "12345 logistream-kvm windows-host.local:9778"
    let mut campos = linha.split_whitespace();
    let Some(pid) = campos.next().and_then(|p| p.parse::<u32>().ok()) else { return };
    let Some(destino) = campos.nth(1) else { return };
    let Some((endereco, porta)) = destino.rsplit_once(':') else { return };
    let Ok(porta) = porta.parse::<u16>() else { return };

    let nome = cfg
        .hosts
        .iter()
        .find(|h| {
            h.addr == endereco && h.screen.as_ref().is_some_and(|s| s.port + 1 == porta)
        })
        .map(|h| h.name.clone())
        .unwrap_or_else(|| endereco.to_string());

    if let Ok(mut g) = ATUAL.lock() {
        *g = Some((nome.clone(), Processo::Adotado(pid)));
        crate::say!("kvm: adotei o processo {pid}, que esta mandando input para {nome}");
    }
}

/// Nos outros sistemas nao ha captura, logo nao ha o que adotar.
#[cfg(not(target_os = "linux"))]
pub fn adotar(_cfg: &Config) {}
