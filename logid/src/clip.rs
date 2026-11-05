//! Clipboard compartilhado entre as máquinas.
//!
//! Spec: ../../specs/004-clipboard.md
//!
//! Substitui o KDE Connect, que era a única peça de terceiros no
//! caminho e trazia um daemon inteiro (descoberta, pareamento,
//! notificações, transferência de arquivos) para usarmos uma função.
//!
//! ## Transporte: SSH, não porta nova
//!
//! A API do `logid` escuta só em `127.0.0.1` por desenho (specs/008).
//! Abrir uma porta na LAN para o clipboard seria superfície nova para
//! um dado que às vezes é senha. SSH já é o canal confiável entre as
//! máquinas -- é por ele que o painel sonda, acorda host de tela e
//! roda o agente -- então o empurrão é `ssh <par> logid clip-set` com
//! o texto no stdin.
//!
//! O preço é ~300 ms por cópia. Para clipboard isso é invisível: você
//! copia, troca de máquina, e só então cola.
//!
//! ## Laço de eco
//!
//! Sem cuidado, A manda para B, B vê o clipboard mudar e manda de
//! volta para A, para sempre. Guardamos o hash do último texto que
//! **vimos ou escrevemos**; mudança que bate com ele não é reenviada.

use std::sync::Mutex;

/// O que pedimos à thread do clipboard para publicar.
///
/// Texto e arquivo não são a mesma coisa com tamanhos diferentes: o
/// texto **é** o conteúdo e vai no corpo do POST; o arquivo é uma
/// referência, e o clipboard do destino tem de apontar para arquivos
/// que existem no disco dele. Ver [`crate::clip_arq`].
#[derive(Debug, Clone)]
pub enum Pedido {
    Texto(String),
    Arquivos(Vec<String>),
}

/// Último texto que passou por aqui, em qualquer direção.
///
/// Guardado inteiro e não só o hash porque o Linux precisa dele para
/// responder `SelectionTransfer` -- quem é dono da seleção tem de
/// conseguir entregá-la quando pedirem.
static ULTIMO: Mutex<Option<String>> = Mutex::new(None);

/// Limite do que viaja. Clipboard de desenvolvedor às vezes carrega um
/// arquivo inteiro colado sem querer, e mandar megabytes por SSH a cada
/// cópia transformaria uma conveniência num engasgo.
pub const LIMITE: usize = 256 * 1024;

pub fn ultimo() -> Option<String> {
    ULTIMO.lock().ok().and_then(|g| g.clone())
}

/// Registra o texto e diz se ele é NOVO.
///
/// Uma função só para as duas direções: tanto "li do clipboard local"
/// quanto "recebi de um par" passam por aqui, e é o `false` que corta
/// o laço de eco.
pub fn registrar(texto: &str) -> bool {
    let Ok(mut g) = ULTIMO.lock() else { return false };
    if g.as_deref() == Some(texto) {
        return false;
    }
    *g = Some(texto.to_string());
    true
}

/// Onde mora o `logid` em cada par, e como invoca-lo.
///
/// Nao da para mandar so `logid clip-set`: o shell nao-interativo do
/// SSH nao carrega o PATH do login, e o Mac respondeu
/// `zsh:1: command not found: logid`. O campo `agent` da config
/// tambem nao serve -- ele aponta para `logid-agent.exe` e so existe
/// no Windows (o Mac o omite de proposito).
///
/// Entao: o caminho que o proprio deploy instala, por sistema.
fn comando_remoto(h: &crate::config::HostCfg) -> String {
    crate::remote::comando_logid(h, "clip-set")
}

/// Empurra o texto para os outros hosts, por SSH.
///
/// Em paralelo, uma thread por par: três máquinas em série seriam três
/// handshakes de SSH somados, e o clipboard ficaria desalinhado por
/// quase um segundo na última.
pub fn empurrar(cfg: &crate::config::Config, texto: &str) {
    let alvos: Vec<(String, String, String)> = cfg
        .hosts
        .iter()
        .filter(|h| {
            !h.local.unwrap_or(false)
                && !matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1")
        })
        .filter_map(|h| h.ssh.clone().map(|s| (h.name.clone(), s, comando_remoto(h))))
        .collect();

    for (nome, destino, comando) in alvos {
        let t = texto.to_string();
        std::thread::spawn(move || {
            use std::io::Write;
            use std::process::Stdio;
            let filho = crate::sys::child("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=6", &destino])
                .arg(&comando)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn();
            let Ok(mut filho) = filho else {
                crate::say!("clip: nao consegui falar com {nome}");
                return;
            };
            if let Some(mut e) = filho.stdin.take() {
                let _ = e.write_all(t.as_bytes());
            }
            match filho.wait_with_output() {
                Ok(o) if o.status.success() => {}
                Ok(o) => crate::say!(
                    "clip: {nome} recusou ({}): {}",
                    o.status,
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => crate::say!("clip: {nome} falhou: {e}"),
            }
        });
    }
}

// --- Linux: sessao de RemoteDesktop do Mutter ------------------------
//
// Sem wl-clipboard e sem portal: o Wayland nao deixa ler a selecao sem
// foco, e o portal `org.freedesktop.portal.Clipboard` exige uma sessao
// com dialogo de permissao. A API privada do Mutter -- a mesma que ja
// usamos para injetar input -- entrega `EnableClipboard`,
// `SelectionRead`, `SetSelection` e os sinais, sem perguntar nada.
//
// Duas pegadinhas, as duas descobertas sondando:
//
// 1. `EnableClipboard` COM `mime-types` nos declara DONOS da selecao,
//    e a leitura passa a falhar com "Tried to read own selection". Para
//    observar, chame sem tipos; declare tipos so quando for escrever.
// 2. O fd de `SelectionRead` vem **nao-bloqueante**. `read_to_end` nele
//    devolve `WouldBlock` na hora -- e quem engolir esse erro le zero
//    bytes e conclui que o clipboard esta vazio. Foi o que aconteceu na
//    primeira sonda.
#[cfg(target_os = "linux")]
pub mod linux {
    use dbus::arg::{RefArg, Variant};
    use dbus::blocking::Connection;
    use dbus::Path;
    use std::collections::HashMap;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::sync::Arc;
    use std::time::Duration;

    const RD: &str = "org.gnome.Mutter.RemoteDesktop";
    const RD_OBJ: &str = "/org/gnome/Mutter/RemoteDesktop";
    const RDS: &str = "org.gnome.Mutter.RemoteDesktop.Session";
    const MIME: &str = "text/plain;charset=utf-8";
    /// Arquivos: a lista de URIs, o que o Wayland e o GTK falam.
    const URI: &str = "text/uri-list";
    /// O que o Nautilus lê ao colar. Primeira linha `copy` ou `cut`.
    const GNOME: &str = "x-special/gnome-copied-files";
    const T: Duration = Duration::from_secs(5);

    /// Le um fd que o Mutter entregou, tirando o O_NONBLOCK antes.
    fn ler_fd(fd: RawFd) -> std::io::Result<Vec<u8>> {
        let copia = unsafe { libc::dup(fd) };
        if copia < 0 {
            return Err(std::io::Error::last_os_error());
        }
        unsafe {
            let fl = libc::fcntl(copia, libc::F_GETFL);
            libc::fcntl(copia, libc::F_SETFL, fl & !libc::O_NONBLOCK);
        }
        let mut f = unsafe { std::fs::File::from_raw_fd(copia) };
        let mut v = Vec::new();
        f.read_to_end(&mut v)?;
        Ok(v)
    }

    fn tipos(quais: &[&str]) -> HashMap<&'static str, Variant<Box<dyn RefArg>>> {
        let mut m: HashMap<&str, Variant<Box<dyn RefArg>>> = HashMap::new();
        let v: Vec<String> = quais.iter().map(|s| s.to_string()).collect();
        m.insert("mime-types", Variant(Box::new(v)));
        m
    }

    /// O que estamos publicando agora, para responder a transferência.
    ///
    /// Quem é dono da seleção tem de conseguir entregá-la quando
    /// pedirem, e o pedido vem por MIME: o Nautilus pede
    /// `x-special/gnome-copied-files`, um terminal pede `text/uri-list`,
    /// um editor pede `text/plain`. Sem guardar o que publicamos, a
    /// colagem chega e não há o que responder.
    static PUBLICADO: std::sync::Mutex<Option<super::Pedido>> = std::sync::Mutex::new(None);

    /// Os bytes que respondem o MIME pedido, se soubermos respondê-lo.
    fn servir(mime: &str) -> Option<Vec<u8>> {
        let g = PUBLICADO.lock().ok()?;
        match g.as_ref()? {
            super::Pedido::Texto(t) => {
                if mime.starts_with("text/plain") || mime == "UTF8_STRING" {
                    Some(t.as_bytes().to_vec())
                } else {
                    None
                }
            }
            super::Pedido::Arquivos(v) => {
                if mime == GNOME {
                    Some(crate::clip_arq::gnome_copied_files(v).into_bytes())
                } else if mime == URI {
                    Some(crate::clip_arq::uris_de_caminhos(v).into_bytes())
                } else if mime.starts_with("text/plain") {
                    // Colar arquivo num editor dá os caminhos. É o que
                    // o GTK faz, e é o que a pessoa espera.
                    Some(v.join("\n").into_bytes())
                } else {
                    None
                }
            }
        }
    }

    /// Lê a seleção atual num MIME, ou `None`.
    ///
    /// O fd vem **não-bloqueante** do Mutter, e `read_to_end` nele
    /// devolve `WouldBlock` na hora -- quem engolir esse erro lê zero
    /// bytes e conclui que o clipboard está vazio. Foi o que aconteceu
    /// na primeira sonda; `ler_fd` tira o O_NONBLOCK.
    fn ler_selecao(c: &Connection, sess: &Path<'static>, mime: &str) -> Option<Vec<u8>> {
        let r: Result<(dbus::arg::OwnedFd,), _> = c
            .with_proxy(RD, sess.clone(), T)
            .method_call(RDS, "SelectionRead", (mime,));
        let (fd,) = r.ok()?;
        ler_fd(fd.as_raw_fd()).ok()
    }

    /// Desce num `RefArg` recolhendo toda string.
    ///
    /// `as_iter` desce **um** nível e a lista vem como `(as)` dentro de
    /// variante, dois níveis abaixo. A primeira versão disto imprimiu
    /// lista vazia e me fez duvidar do Mutter em vez do meu código.
    fn colher(a: &dyn RefArg, fora: &mut Vec<String>) {
        if let Some(s) = a.as_str() {
            fora.push(s.to_string());
            return;
        }
        if let Some(it) = a.as_iter() {
            for x in it {
                colher(&x, fora);
            }
        }
    }

    /// Thread do clipboard: observa, publica e responde transferencias.
    ///
    /// Tudo numa thread so porque a conexao D-Bus **nao e Send** e a
    /// sessao pertence a conexao que a criou -- a mesma restricao que
    /// moldou o host de tela. Quem quer escrever manda pelo canal.
    pub fn spawn(cfg: Arc<std::sync::RwLock<crate::config::Config>>) -> std::sync::mpsc::Sender<super::Pedido> {
        let (tx, rx) = std::sync::mpsc::channel::<super::Pedido>();
        std::thread::spawn(move || {
            if let Err(e) = rodar(&cfg, &rx) {
                crate::say!("clip: desliguei ({e})");
            }
        });
        tx
    }

    fn rodar(
        cfg: &Arc<std::sync::RwLock<crate::config::Config>>,
        rx: &std::sync::mpsc::Receiver<super::Pedido>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let c = Connection::new_session()?;
        let (sess,): (Path,) =
            c.with_proxy(RD, RD_OBJ, T).method_call(RD, "CreateSession", ())?;
        c.with_proxy(RD, sess.clone(), T).method_call::<(), _, _, _>(RDS, "Start", ())?;
        // Sem tipos: queremos OBSERVAR. Declarar tipos aqui nos faria
        // donos da selecao e cegos para a dos outros.
        let vazio: HashMap<&str, Variant<Box<dyn RefArg>>> = HashMap::new();
        c.with_proxy(RD, sess.clone(), T)
            .method_call::<(), _, _, _>(RDS, "EnableClipboard", (vazio,))?;
        crate::say!("clip: observando o clipboard (sessao {sess})");

        // Alguem copiou algo nesta maquina.
        let dono = {
            let s = sess.clone();
            dbus::message::MatchRule::new_signal(RDS, "SelectionOwnerChanged")
                .with_path(s)
        };
        c.add_match(dono, |_: (), _, _| true)?;
        // Pediram o que NOS publicamos.
        let transf = dbus::message::MatchRule::new_signal(RDS, "SelectionTransfer")
            .with_path(sess.clone());
        c.add_match(transf, |_: (), _, _| true)?;

        loop {
            // Escrita pedida por um par, se houver.
            while let Ok(pedido) = rx.try_recv() {
                let quais: &[&str] = match &pedido {
                    super::Pedido::Texto(t) => {
                        if !super::registrar(t) {
                            continue;
                        }
                        &[MIME]
                    }
                    // Os dois tipos juntos: o Nautilus só cola pelo
                    // `gnome-copied-files`, e quem não é GTK só entende
                    // `uri-list`. Publicar um dos dois deixa metade dos
                    // programas sem colar.
                    super::Pedido::Arquivos(_) => &[GNOME, URI, MIME],
                };
                if let Ok(mut g) = PUBLICADO.lock() {
                    *g = Some(pedido);
                }
                let _ = c
                    .with_proxy(RD, sess.clone(), T)
                    .method_call::<(), _, _, _>(RDS, "SetSelection", (tipos(quais),));
            }

            // Uma volta do D-Bus. 300 ms e compromisso: menos que isso
            // gira a CPU a toa, mais que isso atrasa responder uma
            // transferencia -- e quem pediu a selecao fica esperando.
            let msg = c.channel().blocking_pop_message(Duration::from_millis(300))?;
            let Some(m) = msg else { continue };
            let membro = m.member().map(|s| s.to_string()).unwrap_or_default();

            if membro == "SelectionOwnerChanged" {
                // O sinal diz QUAIS tipos o dono publicou, e se o dono
                // somos nós. Medido com `logid clip-sonda`:
                //
                //   texto:   ["text/plain;charset=utf-8", "UTF8_STRING", "TARGETS"]
                //   arquivo: ["text/uri-list", "TARGETS"]
                //
                // Usar essa lista é o único jeito honesto de saber o que
                // foi copiado: `SelectionRead("text/uri-list")` devolve
                // **OK mesmo numa cópia de texto**, então "a leitura
                // funcionou" não distingue nada.
                let opts: HashMap<String, Variant<Box<dyn RefArg>>> =
                    m.read1().unwrap_or_default();
                if opts
                    .get("session-is-owner")
                    .and_then(|v| v.0.as_u64())
                    .is_some_and(|x| x == 1)
                {
                    continue; // é nossa; publicamos nós mesmos
                }
                let mut oferecidos: Vec<String> = Vec::new();
                if let Some(v) = opts.get("mime-types") {
                    colher(&*v.0, &mut oferecidos);
                }
                let tem = |t: &str| oferecidos.iter().any(|x| x == t);

                // Ler na hora: a selecao e do dono ATUAL, e se ele sair
                // antes de lermos o conteudo some.
                if tem(URI) {
                    let Some(bytes) = ler_selecao(&c, &sess, URI) else { continue };
                    let Ok(lista) = String::from_utf8(bytes) else { continue };
                    let caminhos = crate::clip_arq::caminhos_de_uris(&lista);
                    if caminhos.is_empty() {
                        continue;
                    }
                    if let Ok(g) = cfg.read() {
                        crate::clip_arq::ao_copiar(&g, &caminhos);
                    }
                    continue;
                }
                if !oferecidos.is_empty() && !oferecidos.iter().any(|t| t.starts_with("text/plain"))
                {
                    continue; // imagem, HTML, coisa que não tratamos
                }
                let Some(bytes) = ler_selecao(&c, &sess, MIME) else { continue };
                if bytes.is_empty() || bytes.len() > super::LIMITE {
                    continue;
                }
                let Ok(texto) = String::from_utf8(bytes) else { continue };
                if super::registrar(&texto) {
                    if let Ok(g) = cfg.read() {
                        super::empurrar(&g, &texto);
                    }
                }
            } else if membro == "SelectionTransfer" {
                // (mime, serial) -- quem quer colar o que publicamos.
                let (mime, serial): (String, u32) = match m.read2() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                // Responder pelo MIME PEDIDO, e não sempre o mesmo: o
                // Nautilus pede `gnome-copied-files` e um editor pede
                // `text/plain`, da mesma seleção.
                let conteudo = servir(&mime)
                    .unwrap_or_else(|| super::ultimo().unwrap_or_default().into_bytes());
                let r: Result<(dbus::arg::OwnedFd,), _> = c
                    .with_proxy(RD, sess.clone(), T)
                    .method_call(RDS, "SelectionWrite", (serial,));
                let ok = match r {
                    Ok((fd,)) => {
                        use std::io::Write;
                        let copia = unsafe { libc::dup(fd.as_raw_fd()) };
                        let mut f = unsafe { std::fs::File::from_raw_fd(copia) };
                        f.write_all(&conteudo).is_ok()
                    }
                    Err(_) => false,
                };
                let _ = c
                    .with_proxy(RD, sess.clone(), T)
                    .method_call::<(), _, _, _>(RDS, "SelectionWriteDone", (serial, ok));
            }
        }
    }
}

// --- escrita local, por sistema --------------------------------------

/// Escreve no clipboard DESTA maquina.
///
/// No Linux quem escreve e a thread do D-Bus (a sessao pertence a
/// conexao que a criou), entao aqui so empurramos pelo canal. Nos
/// outros dois usamos as ferramentas que o proprio sistema traz:
/// `clip.exe` e `pbcopy` existem em toda instalacao, e escrever
/// clipboard por API nativa seria codigo nosso para fazer o que o
/// sistema ja faz igual.
#[cfg(target_os = "linux")]
pub fn definir_local(canal: &std::sync::mpsc::Sender<Pedido>, texto: &str) -> Result<(), String> {
    canal
        .send(Pedido::Texto(texto.to_string()))
        .map_err(|_| "a thread do clipboard saiu".to_string())
}

#[cfg(not(target_os = "linux"))]
pub fn definir_local(_canal: &(), texto: &str) -> Result<(), String> {
    #[cfg(not(windows))]
    use std::io::Write;
    #[cfg(not(windows))]
    use std::process::Stdio;
    if !registrar(texto) {
        return Ok(()); // ja e esse; escrever de novo so geraria eco
    }
    #[cfg(windows)]
    {
        return winput::clipboard_escrever(texto);
    }
    #[cfg(not(windows))]
    {
        let mut f = crate::sys::child("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("pbcopy: {e}"))?;
        if let Some(mut e) = f.stdin.take() {
            e.write_all(texto.as_bytes()).map_err(|e| format!("pbcopy: {e}"))?;
        }
        f.wait().map_err(|e| format!("pbcopy: {e}"))?;
        Ok(())
    }
}

/// Vigia o clipboard por amostragem. Windows e macOS.
///
/// Por amostragem e nao por evento porque os dois tem API de
/// notificacao (`AddClipboardFormatListener`, `NSPasteboard`
/// `changeCount`) que exigiria laco de mensagens ou objc dentro de um
/// daemon que nao tem nem um nem outro. Dois segundos e o compromisso:
/// nao da para sentir ao trocar de maquina, e sao 30 leituras por
/// minuto de um comando barato.
///
/// O Linux nao passa por aqui -- la o sinal `SelectionOwnerChanged`
/// avisa na hora.
#[cfg(not(target_os = "linux"))]
pub fn vigiar(cfg: std::sync::Arc<std::sync::RwLock<crate::config::Config>>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        // `crate::sys::child`, NUNCA `Command::new` direto.
        //
        // No Windows aquele helper poe `CREATE_NO_WINDOW`; sem ele cada
        // amostragem abre uma janela de console visivel. Com 2 s de
        // intervalo isso virou "o windows ta abrindo um powershell a
        // cada 5s" na tela do usuario -- um processo invisivel que
        // pisca 30 vezes por minuto.
        // Arquivo ANTES de texto, e nao depois.
        //
        // Os dois convivem no clipboard: o Explorer publica `CF_HDROP`
        // junto com o nome do arquivo como texto, e o Finder faz o
        // equivalente. Olhando texto primeiro, copiar um arquivo
        // mandaria o CAMINHO como string para os pares -- um caminho
        // que nao existe la -- e o arquivo nunca iria.
        //
        // No Windows sao duas chamadas Win32, sem processo nenhum. No
        // macOS e UMA chamada que devolve as duas coisas, porque la o
        // custo e o processo: `osascript` custa 55 ms contra 11 ms de
        // `pbpaste` (medido), e rodar os dois a cada 2 s seria 3,3% de
        // um nucleo para sempre num laptop.
        let texto = {
            #[cfg(windows)]
            {
                let arquivos = crate::clip_arq::ler_local();
                if !arquivos.is_empty() {
                    if let Ok(g) = cfg.read() {
                        crate::clip_arq::ao_copiar(&g, &arquivos);
                    }
                    continue;
                }
                match winput::clipboard_ler() {
                    Ok(Some(t)) => t,
                    Ok(None) => continue,
                    Err(_) => continue,
                }
            }
            #[cfg(not(windows))]
            {
                match crate::clip_arq::ler_area() {
                    crate::clip_arq::Area::Arquivos(v) => {
                        if let Ok(g) = cfg.read() {
                            crate::clip_arq::ao_copiar(&g, &v);
                        }
                        continue;
                    }
                    crate::clip_arq::Area::Texto(t) => t,
                    crate::clip_arq::Area::Nada => continue,
                }
            }
        };
        let texto = texto.trim_end_matches(['\r', '\n']).to_string();
        if texto.is_empty() || texto.len() > LIMITE {
            continue;
        }
        if registrar(&texto) {
            if let Ok(g) = cfg.read() {
                empurrar(&g, &texto);
            }
        }
    });
}

/// `logid clip-set`: le o texto do stdin e entrega ao daemon local.
///
/// Passa pela API em 127.0.0.1 em vez de escrever o clipboard direto, e
/// isso e necessario no Linux: quem escreve tem de CONTINUAR VIVO para
/// responder `SelectionTransfer` quando alguem colar. Um processo de
/// CLI que escreve e sai deixaria uma selecao sem dono -- o texto
/// sumiria no primeiro Ctrl+V. Entao o CLI so entrega ao daemon, que
/// fica.
///
/// HTTP cru num TcpStream em vez de um cliente HTTP: sao doze linhas
/// contra uma dependencia inteira, para um POST em localhost.
pub fn cli_set(porta: u16) -> std::process::ExitCode {
    use std::io::{Read, Write};
    let mut texto = String::new();
    if std::io::stdin().read_to_string(&mut texto).is_err() {
        eprintln!("clip-set: nao consegui ler o stdin");
        return std::process::ExitCode::FAILURE;
    }
    if texto.len() > LIMITE {
        eprintln!("clip-set: {} bytes acima do limite de {LIMITE}", texto.len());
        return std::process::ExitCode::FAILURE;
    }
    let corpo = texto.into_bytes();
    let pedido = format!(
        "POST /api/clipboard HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        corpo.len()
    );
    let mut s = match std::net::TcpStream::connect(("127.0.0.1", porta)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("clip-set: o logid local nao respondeu na porta {porta}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if s.write_all(pedido.as_bytes()).is_err() || s.write_all(&corpo).is_err() {
        eprintln!("clip-set: falhei ao escrever o pedido");
        return std::process::ExitCode::FAILURE;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    if resp.starts_with("HTTP/1.1 200") {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("clip-set: {}", resp.lines().next().unwrap_or("sem resposta"));
        std::process::ExitCode::FAILURE
    }
}

// --- ponte entre o handler HTTP e a thread ---------------------------
//
// O handler roda numa das quatro threads do servidor e precisa alcancar
// a thread do clipboard. Guardar o canal num estatico e mais simples
// que carregar mais um `Arc` por toda a assinatura de `handle`, que
// hoje leva apenas config e cache.

#[cfg(target_os = "linux")]
static CANAL: Mutex<Option<std::sync::mpsc::Sender<Pedido>>> = Mutex::new(None);

#[cfg(target_os = "linux")]
pub fn guardar_canal(c: std::sync::mpsc::Sender<Pedido>) {
    if let Ok(mut g) = CANAL.lock() {
        *g = Some(c);
    }
}

/// Escreve no clipboard local o que veio de um par.
pub fn aplicar_local(texto: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let g = CANAL.lock().map_err(|_| "canal travado".to_string())?;
        let c = g.as_ref().ok_or("a thread do clipboard nao subiu")?;
        definir_local(c, texto)
    }
    #[cfg(not(target_os = "linux"))]
    {
        definir_local(&(), texto)
    }
}

/// Publica arquivos no clipboard local (Linux).
///
/// Passa pela thread do D-Bus porque a sessão pertence à conexão que a
/// criou, e porque quem publica tem de continuar vivo para responder
/// `SelectionTransfer` na hora da colagem.
#[cfg(target_os = "linux")]
pub fn pedir_arquivos(caminhos: &[String]) -> Result<(), String> {
    let g = CANAL.lock().map_err(|_| "canal travado".to_string())?;
    let c = g.as_ref().ok_or("a thread do clipboard nao subiu")?;
    c.send(Pedido::Arquivos(caminhos.to_vec()))
        .map_err(|_| "a thread do clipboard saiu".to_string())
}
