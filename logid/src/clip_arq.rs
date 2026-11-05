//! Clipboard de ARQUIVOS entre as máquinas.
//!
//! Spec: ../../specs/004-clipboard.md
//!
//! Irmão de [`crate::clip`], que cuida de texto. Separado porque a
//! diferença não é de tamanho, é de natureza: texto **é** o conteúdo e
//! viaja inteiro no corpo de um POST; arquivo é uma *referência*, e o
//! clipboard do destino tem de apontar para arquivos que existem no
//! disco DELE. Não dá para "colar" um caminho que só existe na máquina
//! de origem.
//!
//! Daí a consequência que molda tudo aqui: os bytes têm de chegar antes
//! da colagem, e não durante. Não há como interceptar o Ctrl+V nos três
//! sistemas -- o Windows exigiria renderização atrasada de `CF_HDROP`, o
//! macOS um provedor de pasteboard vivo -- então copiamos na hora do
//! Ctrl+C e deixamos os arquivos esperando numa pasta de passagem.
//!
//! ## Por que um teto, e o que ele decide
//!
//! Copiar uma pasta num gerenciador de arquivos é um atalho de teclado.
//! Sem teto, selecionar `~/Downloads` sem querer mandaria gigabytes para
//! duas máquinas que você nem está olhando -- e o custo não é só banda,
//! é disco alheio.
//!
//! São três limites, e cada um responde uma pergunta diferente:
//!
//! | Limite | Pergunta |
//! |---|---|
//! | [`TETO_EMBUTIDO`] | vai junto com o aviso, ou precisa de transferência própria? |
//! | [`TETO_TOTAL`] | isto é uma cópia ou um acidente? |
//! | [`TETO_ARQUIVOS`] | idem, para árvore grande e leve (`node_modules`) |
//!
//! ## Transporte
//!
//! `tar` pelo stdin do SSH, nas três máquinas -- o Windows tem
//! `tar.exe` em `system32` desde o 1809. Uma viagem só, sem arquivo
//! temporário no meio, e preserva árvore de diretório.
//!
//! Acima de [`TETO_EMBUTIDO`] preferimos `rsync` **quando existe nas
//! duas pontas**, pelo `--partial`: num arquivo de 2 GB uma queda de
//! Wi-Fi no meio custa a transferência inteira com `tar`, e custa o
//! resto dela com `rsync`. O Windows não tem rsync (medido: só
//! `tar.exe` e `scp.exe`), então lá cai no `tar` e o log diz que não
//! haverá retomada.

use std::path::{Path, PathBuf};

/// Vai embutido no mesmo SSH que avisa o par.
pub const TETO_EMBUTIDO: u64 = 8 * 1024 * 1024;

/// Acima disto não copiamos: avisa e para.
///
/// 2 GiB é generoso de propósito -- mover um vídeo entre máquinas é uso
/// legítimo. O que ele barra é a ordem de magnitude errada: a pasta de
/// usuário inteira, uma biblioteca de fotos, um diretório de VMs.
pub const TETO_TOTAL: u64 = 2 * 1024 * 1024 * 1024;

/// Teto de quantidade, independente de tamanho.
///
/// Existe porque `node_modules` tem 40 mil arquivos e 300 MB: passa
/// folgado no teto de bytes e ainda assim trava a cópia por minutos,
/// um `tar` de 40 mil cabeçalhos.
pub const TETO_ARQUIVOS: usize = 2000;

/// Como os bytes vão viajar -- ou por que não vão.
#[derive(Debug, PartialEq, Eq)]
pub enum Transporte {
    /// `tar` no stdin do SSH, junto com o aviso.
    Embutido,
    /// Transferência própria antes do aviso (`rsync`, ou `tar` onde não
    /// houver).
    Propria,
    /// Não vamos copiar. O texto é o motivo, pronto para o log.
    Recusado(String),
}

/// Decide o transporte a partir do tamanho e da contagem.
///
/// Função pura e testada porque é a regra de negócio deste módulo: é
/// ela que diz "isto é uma cópia" ou "isto é um acidente", e errar para
/// o lado permissivo custa disco e banda de máquina alheia.
pub fn escolher_transporte(total: u64, arquivos: usize) -> Transporte {
    if arquivos == 0 {
        return Transporte::Recusado("nenhum arquivo na seleção".into());
    }
    if arquivos > TETO_ARQUIVOS {
        return Transporte::Recusado(format!(
            "{arquivos} arquivos, acima do teto de {TETO_ARQUIVOS}"
        ));
    }
    if total > TETO_TOTAL {
        return Transporte::Recusado(format!(
            "{} acima do teto de {}",
            humano(total),
            humano(TETO_TOTAL)
        ));
    }
    if total <= TETO_EMBUTIDO {
        Transporte::Embutido
    } else {
        Transporte::Propria
    }
}

/// Tamanho legível. Só para log e mensagem de recusa.
pub fn humano(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / (K * K))
    } else {
        format!("{:.2} GB", f / (K * K * K))
    }
}

/// O que a seleção contém, medido.
#[derive(Debug, Default)]
pub struct Medida {
    pub bytes: u64,
    pub arquivos: usize,
    /// Parou antes de terminar porque já passou de um teto.
    pub estourou: bool,
}

/// Mede a seleção, **parando cedo** quando já estourou um teto.
///
/// Parar cedo não é otimização: sem isso, medir uma seleção que vamos
/// recusar custaria percorrer a árvore inteira primeiro. Apontar o
/// clipboard para `/` levaria minutos de `stat` antes da recusa.
pub fn medir(caminhos: &[String]) -> Medida {
    let mut m = Medida::default();
    for c in caminhos {
        medir_um(Path::new(c), &mut m);
        if m.estourou {
            break;
        }
    }
    m
}

fn medir_um(p: &Path, m: &mut Medida) {
    if m.bytes > TETO_TOTAL || m.arquivos > TETO_ARQUIVOS {
        m.estourou = true;
        return;
    }
    let Ok(meta) = std::fs::symlink_metadata(p) else { return };
    if meta.is_dir() {
        let Ok(it) = std::fs::read_dir(p) else { return };
        for e in it.flatten() {
            medir_um(&e.path(), m);
            if m.estourou {
                return;
            }
        }
    } else {
        // Link simbólico conta como entrada mas não como o alvo: seguir
        // links faria uma seleção de 3 KB medir o disco inteiro, e um
        // ciclo de links não terminaria nunca.
        m.arquivos += 1;
        m.bytes += meta.len();
    }
}

// --- text/uri-list ---------------------------------------------------
//
// O formato que o Wayland, o GTK e o Finder falam quando o assunto é
// arquivo no clipboard. Uma URI por linha, terminada em CRLF pela RFC
// 2483, e caminho percent-encoded.
//
// O encoding é o detalhe que quebra na prática: nome com espaço, com
// acento, com `#`. `arquivo final.pdf` virando `arquivo` + `final.pdf`
// é o bug clássico desta conversão, e é por isso que ela é pura e tem
// teste.

/// Caracteres que podem ir crus numa URI de arquivo.
///
/// `:` entra porque e `pchar` valido na RFC 3986 e porque e o que o
/// Explorer e o GTK produzem: `file:///C:/Users/...`. Escapa-lo daria
/// `C%3A`, que funciona na nossa volta mas nao e o que outro programa
/// espera encontrar no clipboard.
///
/// `#` e `?` NAO entram: um inicia fragmento e o outro query, e um
/// arquivo chamado `nota #3.txt` perderia metade do nome.
fn livre(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// Caminhos locais -> `text/uri-list`.
pub fn uris_de_caminhos(caminhos: &[String]) -> String {
    let mut s = String::new();
    for c in caminhos {
        // Tres barras quando o caminho nao comeca em `/`.
        //
        // `file://C:/Users/x` poe `C:` na AUTORIDADE da URI -- isto e,
        // declara um host chamado `C` na porta vazia -- e o caminho
        // vira `/Users/x`. O arquivo some e o erro nao se parece com
        // um erro de caminho. O certo e `file:///C:/Users/x`.
        s.push_str(if c.starts_with('/') { "file://" } else { "file:///" });
        for &b in c.as_bytes() {
            if livre(b) {
                s.push(b as char);
            } else if b == b'\\' {
                // Caminho do Windows vira URI com barra normal, senão o
                // destino recebe `C:%5CUsers` e não acha nada.
                s.push('/');
            } else {
                s.push_str(&format!("%{b:02X}"));
            }
        }
        s.push_str("\r\n");
    }
    s
}

/// `text/uri-list` -> caminhos locais.
///
/// Ignora linha em branco e comentário (`#`, pela RFC) e descarta
/// qualquer esquema que não seja `file://` -- um `http://` no
/// clipboard é um link, não um arquivo, e tratá-lo como caminho criaria
/// um arquivo chamado `http:` na pasta de passagem.
pub fn caminhos_de_uris(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    for linha in s.lines() {
        let l = linha.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some(resto) = l.strip_prefix("file://") else { continue };
        // `file://host/caminho` existe na RFC; só aceitamos host vazio
        // ou `localhost`, porque um host de verdade seria um arquivo
        // que não está aqui.
        let resto = resto.strip_prefix("localhost").unwrap_or(resto);
        if !resto.starts_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        let b = resto.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(x) => {
                        bytes.push(x);
                        i += 3;
                        continue;
                    }
                    // `%` solto é `%` literal, e não motivo para
                    // descartar o caminho inteiro.
                    None => {}
                }
            }
            bytes.push(b[i]);
            i += 1;
        }
        if let Ok(c) = String::from_utf8(bytes) {
            v.push(normalizar_windows(&c));
        }
    }
    v
}

/// `/C:/Users/x` -> `C:\Users\x`.
///
/// A URI de arquivo do Windows tem uma barra a mais antes da letra do
/// drive, e quem recebe precisa do caminho nativo para abrir.
fn normalizar_windows(c: &str) -> String {
    let b = c.as_bytes();
    if b.len() >= 4 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return c[1..].replace('/', "\\");
    }
    c.to_string()
}

// --- x-special/gnome-copied-files ------------------------------------

/// O que o Nautilus (e o GTK em geral) lê ao colar.
///
/// Primeira linha `copy` ou `cut`, depois as URIs. **Sempre `copy`**,
/// nunca `cut`: `cut` manda o destino apagar a origem depois de colar,
/// e aqui a origem está em outra máquina. Não temos como saber que a
/// colagem aconteceu, e apagar arquivo de alguém por engano não tem
/// desfazer.
pub fn gnome_copied_files(caminhos: &[String]) -> String {
    format!("copy\n{}", uris_de_caminhos(caminhos).replace("\r\n", "\n"))
}

// --- pasta de passagem ------------------------------------------------

/// Onde os arquivos recebidos esperam a colagem.
///
/// Fora de `/tmp` de propósito: `/tmp` é limpo no boot em algumas
/// distros e vive em tmpfs (RAM) em outras, e 2 GB de clipboard na RAM
/// é um jeito de descobrir o OOM killer.
pub fn pasta_de_passagem() -> PathBuf {
    #[cfg(windows)]
    {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
        Path::new(&base).join("logid").join("clipboard")
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Path::new(&base).join(".cache").join("logi").join("clipboard")
    }
}

/// Quantas entregas guardamos antes de apagar as antigas.
pub const ENTREGAS_GUARDADAS: usize = 5;

/// Apaga entregas antigas, mantendo as [`ENTREGAS_GUARDADAS`] últimas.
///
/// Sem isto a pasta cresce para sempre: cada Ctrl+C de arquivo deixa
/// uma cópia, e ninguém vai lá limpar.
pub fn podar(pasta: &Path) {
    let Ok(it) = std::fs::read_dir(pasta) else { return };
    let mut dirs: Vec<PathBuf> = it
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    // Nome é o milissegundo da entrega, então ordem alfabética de
    // número zero-preenchido é ordem cronológica.
    dirs.sort();
    let sobra = dirs.len().saturating_sub(ENTREGAS_GUARDADAS);
    for d in dirs.into_iter().take(sobra) {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Nome da entrega: milissegundos, zero-preenchido para ordenar.
pub fn nome_de_entrega(agora_ms: u128) -> String {
    format!("{agora_ms:014}")
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn teto_separa_copia_de_acidente() {
        assert_eq!(escolher_transporte(1024, 1), Transporte::Embutido);
        assert_eq!(escolher_transporte(TETO_EMBUTIDO, 3), Transporte::Embutido);
        assert_eq!(escolher_transporte(TETO_EMBUTIDO + 1, 3), Transporte::Propria);
        assert!(matches!(
            escolher_transporte(TETO_TOTAL + 1, 3),
            Transporte::Recusado(_)
        ));
        // Arvore grande e leve: passa no teto de bytes e e barrada pela
        // contagem. O caso `node_modules`.
        assert!(matches!(
            escolher_transporte(300 * 1024 * 1024, 40_000),
            Transporte::Recusado(_)
        ));
        assert!(matches!(escolher_transporte(0, 0), Transporte::Recusado(_)));
    }

    #[test]
    fn a_recusa_diz_o_motivo_em_numero() {
        // Mensagem de recusa sem numero obriga a pessoa a adivinhar o
        // que reduzir.
        let Transporte::Recusado(m) = escolher_transporte(3 * 1024 * 1024 * 1024, 2) else {
            panic!("devia recusar");
        };
        assert!(m.contains("3.00 GB"), "{m}");
        assert!(m.contains("2.00 GB"), "{m}");
    }

    #[test]
    fn espaco_e_acento_sobrevivem_a_ida_e_volta() {
        // O bug classico: "arquivo final.pdf" virando dois caminhos.
        let orig = vec![
            "/home/user/arquivo final.pdf".to_string(),
            "/home/user/relatório #3.txt".to_string(),
        ];
        let lista = uris_de_caminhos(&orig);
        assert!(lista.contains("arquivo%20final.pdf"), "{lista}");
        assert!(!lista.contains('#'), "o # tem de ser escapado: {lista}");
        assert_eq!(caminhos_de_uris(&lista), orig);
    }

    #[test]
    fn caminho_do_windows_vai_e_volta() {
        let w = vec![r"C:\Users\windows-host\nota.txt".to_string()];
        let lista = uris_de_caminhos(&w);
        assert_eq!(lista.trim_end(), "file:///C:/Users/windows-host/nota.txt");
        assert_eq!(caminhos_de_uris(&lista), w, "ida e volta no Windows");
        // Na volta, o `/C:/` da URI tem de virar `C:\` nativo.
        assert_eq!(caminhos_de_uris("file:///C:/Users/x/a.txt"), vec![r"C:\Users\x\a.txt"]);
    }

    #[test]
    fn descarta_o_que_nao_e_arquivo() {
        let s = "# comentario\r\nhttp://exemplo.com/a.pdf\r\n\r\nfile:///tmp/ok.txt\r\n";
        assert_eq!(caminhos_de_uris(s), vec!["/tmp/ok.txt"]);
    }

    #[test]
    fn aceita_localhost_e_recusa_host_de_verdade() {
        assert_eq!(caminhos_de_uris("file://localhost/tmp/a"), vec!["/tmp/a"]);
        // Arquivo noutra maquina nao e arquivo aqui.
        assert!(caminhos_de_uris("file://outra-maquina/tmp/a").is_empty());
    }

    #[test]
    fn porcento_solto_nao_descarta_o_caminho() {
        assert_eq!(caminhos_de_uris("file:///tmp/100%/a.txt"), vec!["/tmp/100%/a.txt"]);
    }

    #[test]
    fn nunca_manda_cut() {
        // `cut` faria o destino apagar a origem, que esta noutra
        // maquina. Nao ha desfazer para isso.
        let s = gnome_copied_files(&["/tmp/a".to_string()]);
        assert!(s.starts_with("copy\n"), "{s}");
        assert!(!s.contains("cut"), "{s}");
    }

    #[test]
    fn entrega_ordena_cronologicamente_como_texto() {
        // `podar` confia nisto: ordem alfabetica == ordem do tempo.
        let mut v = [nome_de_entrega(1762300000000), nome_de_entrega(999), nome_de_entrega(1762300000001)];
        v.sort();
        assert_eq!(v[0], nome_de_entrega(999));
        assert_eq!(v[2], nome_de_entrega(1762300000001));
    }

    #[test]
    fn medir_para_cedo_em_arvore_grande() {
        // Nao percorrer o que vamos recusar. Arvore sintetica rasa com
        // mais arquivos que o teto.
        let t = std::env::temp_dir().join(format!("logi-medir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        for i in 0..(TETO_ARQUIVOS + 50) {
            std::fs::write(t.join(format!("f{i}")), b"x").unwrap();
        }
        let m = medir(&[t.to_string_lossy().to_string()]);
        assert!(m.estourou, "devia ter parado cedo");
        assert!(m.arquivos <= TETO_ARQUIVOS + 2, "parou tarde: {}", m.arquivos);
        let _ = std::fs::remove_dir_all(&t);
    }
}

// --- sonda do clipboard (Linux) --------------------------------------

/// Desce num `RefArg` recolhendo toda string que encontrar.
///
/// `as_iter` numa `Variant` desce **um** nível, e um `as` dentro de
/// variante está dois níveis abaixo -- foi por isso que a primeira
/// sonda imprimiu lista vazia e me fez duvidar do Mutter em vez do meu
/// código.
#[cfg(target_os = "linux")]
fn colher_strings(a: &dyn dbus::arg::RefArg, fora: &mut Vec<String>) {
    if let Some(s) = a.as_str() {
        fora.push(s.to_string());
        return;
    }
    if let Some(it) = a.as_iter() {
        for x in it {
            colher_strings(&x, fora);
        }
    }
}

/// `logid clip-sonda`: mostra o que o Mutter oferece a cada cópia.
///
/// Existe porque a pergunta que decide o desenho do lado Linux não se
/// responde lendo documentação: o sinal `SelectionOwnerChanged` diz
/// quais MIME types o dono publicou? E pedir `text/uri-list` quando o
/// dono só tem texto devolve erro limpo, ou pendura?
///
/// Rodar, copiar um texto, copiar um arquivo no Nautilus, comparar.
#[cfg(target_os = "linux")]
pub fn cli_sonda() -> std::process::ExitCode {
    use dbus::arg::{RefArg, Variant};
    use dbus::blocking::Connection;
    use dbus::Path;
    use std::collections::HashMap;
    use std::time::Duration;

    const RD: &str = "org.gnome.Mutter.RemoteDesktop";
    const RD_OBJ: &str = "/org/gnome/Mutter/RemoteDesktop";
    const RDS: &str = "org.gnome.Mutter.RemoteDesktop.Session";
    let t = Duration::from_secs(5);

    let r = (|| -> Result<(), Box<dyn std::error::Error>> {
        let c = Connection::new_session()?;
        let (sess,): (Path,) = c.with_proxy(RD, RD_OBJ, t).method_call(RD, "CreateSession", ())?;
        c.with_proxy(RD, sess.clone(), t).method_call::<(), _, _, _>(RDS, "Start", ())?;
        let vazio: HashMap<&str, Variant<Box<dyn RefArg>>> = HashMap::new();
        c.with_proxy(RD, sess.clone(), t)
            .method_call::<(), _, _, _>(RDS, "EnableClipboard", (vazio,))?;
        println!("observando (sessao {sess}). copie algo; ctrl-c para sair.\n");

        let regra = dbus::message::MatchRule::new_signal(RDS, "SelectionOwnerChanged")
            .with_path(sess.clone());
        c.add_match(regra, |_: (), _, _| true)?;

        loop {
            let Some(m) = c.channel().blocking_pop_message(Duration::from_millis(400))? else {
                continue;
            };
            if m.member().map(|s| s.to_string()).as_deref() != Some("SelectionOwnerChanged") {
                continue;
            }
            // Os argumentos do sinal, como vierem.
            match m.read1::<HashMap<String, Variant<Box<dyn RefArg>>>>() {
                Ok(opts) => {
                    let chaves: Vec<&String> = opts.keys().collect();
                    println!("sinal com as chaves {chaves:?}");
                    if let Some(v) = opts.get("mime-types") {
                        println!("  assinatura: {}", v.0.signature());
                        let mut tipos = Vec::new();
                        colher_strings(&*v.0, &mut tipos);
                        println!("  mime-types: {tipos:?}");
                    }
                    if let Some(v) = opts.get("session-is-owner") {
                        println!("  session-is-owner: {:?}", v.0.as_u64().map(|x| x == 1));
                    }
                }
                Err(e) => println!("sinal sem dict legivel ({e})"),
            }
            // A outra metade da pergunta: pedir uri-list sempre.
            let agora = std::time::Instant::now();
            let r: Result<(dbus::arg::OwnedFd,), _> = c
                .with_proxy(RD, sess.clone(), t)
                .method_call(RDS, "SelectionRead", ("text/uri-list",));
            match r {
                Ok(_) => println!("  SelectionRead(uri-list): OK em {:?}\n", agora.elapsed()),
                Err(e) => println!(
                    "  SelectionRead(uri-list): erro em {:?} -- {}\n",
                    agora.elapsed(),
                    e.message().unwrap_or("sem mensagem")
                ),
            }
        }
    })();
    if let Err(e) = r {
        eprintln!("clip-sonda: {e}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

// --- eco -------------------------------------------------------------

/// Última lista que passou por aqui, em qualquer direção.
static ULTIMA: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// Registra a lista e diz se ela é NOVA.
///
/// Mesmo papel de [`crate::clip::registrar`]: é o `false` que corta o
/// laço de A manda para B, B vê mudar e manda de volta.
///
/// Compara por **nome de arquivo**, não por caminho completo: o destino
/// escreve `~/.cache/logi/clipboard/.../nota.txt` enquanto a origem tem
/// `~/dev/nota.txt`, e comparar caminho faria todo recebimento parecer
/// cópia nova -- ou seja, eco infinito entre três máquinas.
pub fn registrar(caminhos: &[String]) -> bool {
    let chave: Vec<String> = caminhos.iter().map(|c| base_de(c)).collect();
    let Ok(mut g) = ULTIMA.lock() else { return false };
    if g.as_deref() == Some(&chave[..]) {
        return false;
    }
    *g = Some(chave);
    true
}

/// Último componente do caminho, com separador dos dois mundos.
pub fn base_de(c: &str) -> String {
    c.rsplit(['/', '\\']).next().unwrap_or(c).to_string()
}

/// Diretório que contém o caminho. Vazio se não houver.
pub fn pai_de(c: &str) -> String {
    match c.rfind(['/', '\\']) {
        Some(0) => "/".to_string(),
        Some(i) => c[..i].to_string(),
        None => String::new(),
    }
}

// --- ler o clipboard local (Windows e macOS) -------------------------
//
// O Linux não passa por aqui: lá a leitura nasce do sinal do Mutter,
// dentro da thread que é dona da sessão D-Bus.

/// Script JXA que LÊ as URLs de arquivo do NSPasteboard.
///
/// JXA e não AppleScript porque `set the clipboard to {POSIX file ...}`
/// publica um tipo opaco (`'list'`) e **não** `public.file-url` --
/// medido: o Finder não colaria. `readObjectsForClasses` é a API certa,
/// e `isFileURL` filtra o caso de um link `http` copiado, que senão
/// viraria um "arquivo" chamado `https:`.
///
/// Devolve as DUAS coisas numa chamada -- `A\n<caminhos>` ou
/// `T\n<texto>` -- porque o custo aqui é o processo, não o trabalho:
/// medido, 55 ms por `osascript` contra 11 ms de `pbpaste`. A cada 2 s,
/// rodar os dois seria 3,3% de um núcleo para sempre num laptop. Com a
/// leitura unificada o macOS gasta UM processo por amostragem, menos do
/// que gastava antes desta feature existir.
#[cfg(target_os = "macos")]
const LER_PB_JS: &str = r#"ObjC.import("AppKit");const pb=$.NSPasteboard.generalPasteboard;const o=pb.readObjectsForClassesOptions($([$.NSURL]),$({}));const r=[];if(!o.isNil()){for(let i=0;i<o.count;i++){const u=o.objectAtIndex(i);if(u.isFileURL)r.push(u.path.js)}}r.length?"A\n"+r.join("\n"):"T\n"+($.NSString.stringWithString(pb.stringForType($.NSPasteboardTypeString)||"").js)"#;

/// Script JXA que ESCREVE URLs de arquivo no NSPasteboard.
///
/// Os caminhos chegam por **argv**, e não interpolados no script: nome
/// de arquivo carrega aspas, barras e acento, e montar JavaScript por
/// concatenação de string é como esse tipo de código quebra.
#[cfg(target_os = "macos")]
const ESCREVER_PB_JS: &str = r#"function run(argv){ObjC.import("AppKit");const pb=$.NSPasteboard.generalPasteboard;pb.clearContents;const u=argv.map(p=>$.NSURL.fileURLWithPath(p));return pb.writeObjects($(u))?"ok":"falhou"}"#;

/// Grava um script em disco para o `osascript` ler.
///
/// Em arquivo e não em `-e` porque o script tem `function run(argv)` e
/// aspas duplas por tudo; passar isso por linha de comando é convite a
/// bug de quoting -- o projeto já perdeu tempo com três deles na ponte
/// do PowerShell.
#[cfg(target_os = "macos")]
fn script_em_disco(nome: &str, corpo: &str) -> Result<PathBuf, String> {
    // No PAI da pasta de passagem: dentro dela, o script apareceria
    // junto das entregas e `entradas()` o mandaria para o clipboard.
    let dir = pasta_de_passagem().parent().map(|p| p.to_path_buf()).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let p = dir.join(nome);
    std::fs::write(&p, corpo).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

/// Os arquivos que estão no clipboard DESTA máquina, se houver.
#[cfg(windows)]
pub fn ler_local() -> Vec<String> {
    winput::clipboard_ler_arquivos().ok().flatten().unwrap_or_default()
}

/// O que está na área de transferência do Mac: arquivos, texto, ou nada.
#[cfg(target_os = "macos")]
pub enum Area {
    Arquivos(Vec<String>),
    Texto(String),
    Nada,
}

#[cfg(target_os = "macos")]
pub fn ler_area() -> Area {
    let Ok(js) = script_em_disco("ler-pb.js", LER_PB_JS) else { return Area::Nada };
    let Ok(o) = crate::sys::child("osascript").args(["-l", "JavaScript"]).arg(&js).output() else {
        return Area::Nada;
    };
    if !o.status.success() {
        return Area::Nada;
    }
    let saida = String::from_utf8_lossy(&o.stdout);
    let mut linhas = saida.lines();
    match linhas.next() {
        Some("A") => {
            let v: Vec<String> = linhas
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            if v.is_empty() {
                Area::Nada
            } else {
                Area::Arquivos(v)
            }
        }
        // O texto pode ter quebra de linha, então junta o resto de
        // volta em vez de pegar só a primeira.
        Some("T") => {
            let t: Vec<&str> = linhas.collect();
            let t = t.join("\n");
            if t.trim().is_empty() {
                Area::Nada
            } else {
                Area::Texto(t)
            }
        }
        _ => Area::Nada,
    }
}

#[cfg(target_os = "macos")]
pub fn ler_local() -> Vec<String> {
    match ler_area() {
        Area::Arquivos(v) => v,
        _ => Vec::new(),
    }
}

#[cfg(target_os = "linux")]
pub fn ler_local() -> Vec<String> {
    Vec::new()
}

/// Põe os arquivos no clipboard DESTA máquina.
#[cfg(windows)]
pub fn escrever_local(caminhos: &[String]) -> Result<(), String> {
    winput::clipboard_escrever_arquivos(caminhos)
}

#[cfg(target_os = "macos")]
pub fn escrever_local(caminhos: &[String]) -> Result<(), String> {
    let js = script_em_disco("escrever-pb.js", ESCREVER_PB_JS)?;
    let o = crate::sys::child("osascript")
        .args(["-l", "JavaScript"])
        .arg(&js)
        .args(caminhos)
        .output()
        .map_err(|e| format!("osascript: {e}"))?;
    if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ok") {
        Ok(())
    } else {
        Err(format!(
            "osascript recusou: {}{}",
            String::from_utf8_lossy(&o.stdout).trim(),
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

// --- empurrar para os pares ------------------------------------------

/// Seleção grande esperando destino escolhido a mão.
///
/// Acima de [`TETO_EMBUTIDO`] **não** mandamos para todo mundo: copiar
/// 1,4 GB para duas máquinas que você não está olhando gasta banda e
/// disco alheio para nada. A seleção fica registrada aqui e a bandeja
/// oferece "mandar para <host>", um destino por vez.
static OFERTA: std::sync::Mutex<Option<(Vec<String>, u64)>> = std::sync::Mutex::new(None);

/// A seleção grande pendente, se houver: (caminhos, bytes).
pub fn oferta() -> Option<(Vec<String>, u64)> {
    OFERTA.lock().ok().and_then(|g| g.clone())
}

struct Par {
    nome: String,
    ssh: String,
    /// O host, guardado para montar o comando remoto na hora.
    cfg: crate::config::HostCfg,
    windows: bool,
}

fn pares(cfg: &crate::config::Config) -> Vec<Par> {
    cfg.hosts
        .iter()
        .filter(|h| {
            !h.local.unwrap_or(false)
                && !matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1")
        })
        .filter_map(|h| {
            h.ssh.clone().map(|s| Par {
                nome: h.name.clone(),
                ssh: s,
                cfg: h.clone(),
                windows: h.os.as_deref() == Some("windows"),
            })
        })
        .collect()
}

/// Chamada quando alguém copiou arquivos NESTA máquina.
///
/// Decide entre mandar na hora, oferecer, ou recusar -- e **diz** qual
/// das três no log. Uma cópia que silenciosamente não vai para lado
/// nenhum é pior que uma recusa explicada: você descobre na hora de
/// colar, do outro lado, sem pista do motivo.
pub fn ao_copiar(cfg: &crate::config::Config, caminhos: &[String]) {
    if !registrar(caminhos) {
        return; // é o que já temos; mandar de novo é eco
    }
    // Dois arquivos com o mesmo nome base viriam um sobre o outro na
    // pasta de passagem, e a pessoa colaria um arquivo achando que são
    // dois. Recusar é honesto; renomear em silêncio não é.
    let mut bases: Vec<String> = caminhos.iter().map(|c| base_de(c)).collect();
    bases.sort();
    let antes = bases.len();
    bases.dedup();
    if bases.len() != antes {
        crate::say!("clip-arq: dois arquivos com o mesmo nome na seleção; não mandei");
        return;
    }

    let m = medir(caminhos);
    match escolher_transporte(m.bytes, m.arquivos) {
        Transporte::Recusado(motivo) => {
            if let Ok(mut g) = OFERTA.lock() {
                *g = None;
            }
            crate::say!("clip-arq: {motivo}; não mandei");
        }
        Transporte::Embutido => {
            if let Ok(mut g) = OFERTA.lock() {
                *g = None;
            }
            let n = m.arquivos;
            crate::say!("clip-arq: mandando {n} arquivo(s), {}", humano(m.bytes));
            for p in pares(cfg) {
                let c = caminhos.to_vec();
                std::thread::spawn(move || {
                    match mandar_tar(&p, &c) {
                        Ok(()) => crate::say!("clip-arq: {} recebeu", p.nome),
                        Err(e) => crate::say!("clip-arq: {} não recebeu -- {e}", p.nome),
                    }
                });
            }
        }
        Transporte::Propria => {
            if let Ok(mut g) = OFERTA.lock() {
                *g = Some((caminhos.to_vec(), m.bytes));
            }
            crate::say!(
                "clip-arq: {} em {} arquivo(s) -- grande para ir sozinho; escolha o destino na bandeja",
                humano(m.bytes),
                m.arquivos
            );
        }
    }
}

/// Monta os pares `-C <pai> <base>` do tar.
///
/// Um `-C` por arquivo porque a seleção pode vir de pastas diferentes,
/// e o `tar` aceita vários (testado no GNU tar e no bsdtar do Windows).
/// O arquivo entra no pacote pelo nome base, então a árvore do destino
/// não reproduz `/home/user/dev/...` -- a pasta de passagem fica
/// plana e colável.
pub fn args_do_tar(caminhos: &[String]) -> Vec<String> {
    let mut v = Vec::new();
    for c in caminhos {
        let pai = pai_de(c);
        if pai.is_empty() {
            continue;
        }
        v.push("-C".into());
        v.push(pai);
        v.push(base_de(c));
    }
    v
}

/// `tar -cf - ... | ssh par logid clip-arq-set`
///
/// O receptor **não recebe a lista de nomes**. Depois de desempacotar,
/// as entradas de primeiro nível da pasta nova *são* a seleção -- e
/// isso evita passar nome de arquivo com espaço e acento por cima de um
/// shell remoto que, no Windows, é PowerShell. Três bugs de quoting
/// nessa ponte já custaram tempo (specs/002).
fn mandar_tar(p: &Par, caminhos: &[String]) -> Result<(), String> {
    use std::process::Stdio;
    let mut tar = crate::sys::child("tar")
        .arg("-cf")
        .arg("-")
        .args(args_do_tar(caminhos))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("tar: {e}"))?;
    let saida = tar.stdout.take().ok_or("tar sem stdout")?;
    let ssh = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", &p.ssh])
        .arg(crate::remote::comando_logid(&p.cfg, "clip-arq-set"))
        .stdin(Stdio::from(saida))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ssh: {e}"))?;
    let o = ssh.wait_with_output().map_err(|e| format!("ssh: {e}"))?;
    let _ = tar.wait();
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {}",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

/// Manda a seleção pendente para UM host. É o que a bandeja chama.
pub fn mandar_oferta(cfg: &crate::config::Config, host: &str) -> Result<String, String> {
    let (caminhos, bytes) = oferta().ok_or("não há seleção grande pendente")?;
    let p = pares(cfg)
        .into_iter()
        .find(|p| p.nome == host)
        .ok_or_else(|| format!("host '{host}' não está na config ou não tem ssh"))?;
    // `rsync` só onde existe nas duas pontas -- o Windows tem `tar.exe`
    // e `scp.exe` em `system32`, mas não rsync (medido). Vale a
    // distinção porque é aqui que `--partial` paga: numa queda de
    // Wi-Fi no meio de 2 GB, o `tar` perde a transferência inteira e o
    // `rsync` perde só o resto.
    let usar_rsync = !p.windows && !cfg!(windows);
    let r = if usar_rsync {
        mandar_rsync(&p, &caminhos)
    } else {
        mandar_tar(&p, &caminhos)
    };
    match r {
        Ok(()) => {
            let via = if usar_rsync { "rsync" } else { "tar" };
            crate::say!("clip-arq: {} foram para {} via {via}", humano(bytes), p.nome);
            Ok(format!("{} enviados para {}", humano(bytes), p.nome))
        }
        Err(e) => Err(format!("{}: {e}", p.nome)),
    }
}

/// rsync com retomada, em três passos.
///
/// Precisa dos três porque o rsync não cria a pasta de destino
/// aninhada: o par abre a pasta e diz o caminho, o rsync preenche, e
/// um terceiro SSH avisa que chegou. São três handshakes contra um do
/// `tar`, e isso só se paga em transferência grande -- que é
/// exatamente onde esta função é usada.
fn mandar_rsync(p: &Par, caminhos: &[String]) -> Result<(), String> {
    let destino = &p.ssh;
    let o = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", destino])
        .arg(crate::remote::comando_logid(&p.cfg, "clip-arq-dir"))
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    if !o.status.success() {
        return Err(format!("clip-arq-dir: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let dir = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if dir.is_empty() {
        return Err("o par não respondeu o caminho da pasta".into());
    }

    let o = crate::sys::child("rsync")
        .args(["-a", "--partial", "--append-verify"])
        .args(caminhos)
        .arg(format!("{destino}:{dir}/"))
        .output()
        .map_err(|e| format!("rsync: {e}"))?;
    if !o.status.success() {
        return Err(format!("rsync: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }

    let o = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", destino])
        .arg(crate::remote::comando_logid(&p.cfg, &format!("clip-arq-prontos {dir}")))
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("clip-arq-prontos: {}", String::from_utf8_lossy(&o.stderr).trim()))
    }
}

// --- lado que RECEBE --------------------------------------------------

/// Abre uma pasta de entrega nova e imprime o caminho.
///
/// `logid clip-arq-dir`, chamado pelo rsync da origem antes de copiar.
pub fn cli_dir() -> std::process::ExitCode {
    match nova_entrega() {
        Ok(d) => {
            println!("{}", d.display());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("clip-arq-dir: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn nova_entrega() -> Result<PathBuf, String> {
    let base = pasta_de_passagem();
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    podar(&base);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let d = base.join(nome_de_entrega(ms));
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    Ok(d)
}

/// O que ficou na pasta de entrega, em caminho absoluto.
///
/// É isto que substitui mandar a lista de nomes pela linha de comando:
/// depois do `tar`, o que está no primeiro nível da pasta *é* a
/// seleção. Nome com espaço, acento ou aspas nunca passa por um shell.
fn entradas(dir: &Path) -> Vec<String> {
    let Ok(it) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<String> = it
        .flatten()
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

/// `logid clip-arq-set`: desempacota o tar do stdin e põe no clipboard.
///
/// Lê o stdin **neste processo** e grava um arquivo, em vez de deixar o
/// `tar` herdar o stdin do SSH. A versão com `Stdio::inherit` parecia
/// melhor -- um buffer a menos -- e no Windows entregava pasta vazia
/// com exit 0, sem mensagem nenhuma: o `logid.exe` é
/// `windows_subsystem = "windows"` (sem console), e nessa condição o
/// filho não recebia o stdin que o processo pai recebe sem problema. O
/// `clip-set` de texto funciona justamente porque lê o stdin dentro do
/// processo -- então é esse o caminho provado, e é o que usamos.
///
/// Guardar tudo em memória é seguro porque este caminho só existe
/// abaixo de [`TETO_EMBUTIDO`]; acima dele a transferência é própria.
/// A folga de 1 MiB cobre o cabeçalho do tar e o padding de bloco.
pub fn cli_set(porta: u16) -> std::process::ExitCode {
    use std::io::Read;
    let teto = TETO_EMBUTIDO + 1024 * 1024;
    let mut pacote = Vec::new();
    // `take`: um stream maior que o teto para aqui e não na RAM.
    if let Err(e) = std::io::stdin().take(teto + 1).read_to_end(&mut pacote) {
        eprintln!("clip-arq-set: nao consegui ler o stdin: {e}");
        return std::process::ExitCode::FAILURE;
    }
    if pacote.is_empty() {
        eprintln!("clip-arq-set: stdin vazio -- nenhum pacote chegou");
        return std::process::ExitCode::FAILURE;
    }
    if pacote.len() as u64 > teto {
        eprintln!("clip-arq-set: pacote acima de {}", humano(teto));
        return std::process::ExitCode::FAILURE;
    }
    let dir = match nova_entrega() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("clip-arq-set: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Fora da pasta de entrega: dentro, o próprio .tar apareceria em
    // `entradas()` e iria para o clipboard junto com os arquivos.
    let tar_path = pasta_de_passagem().join(format!("{}.tar", nome_de_entrega(0)));
    if let Err(e) = std::fs::write(&tar_path, &pacote) {
        eprintln!("clip-arq-set: {}: {e}", tar_path.display());
        return std::process::ExitCode::FAILURE;
    }
    let r = crate::sys::child("tar").arg("-xf").arg(&tar_path).arg("-C").arg(&dir).output();
    let _ = std::fs::remove_file(&tar_path);
    match r {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!("clip-arq-set: tar falhou -- {}", String::from_utf8_lossy(&o.stderr).trim());
            return std::process::ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("clip-arq-set: tar: {e}");
            return std::process::ExitCode::FAILURE;
        }
    }
    entregar(porta, &dir)
}

/// `logid clip-arq-prontos <dir>`: os arquivos já chegaram (rsync).
pub fn cli_prontos(porta: u16, dir: &str) -> std::process::ExitCode {
    let d = PathBuf::from(dir);
    // Só dentro da pasta de passagem. Sem isto, um `clip-arq-prontos /`
    // apontaria o clipboard para a raiz -- e quem chama vem da rede.
    if !d.starts_with(pasta_de_passagem()) {
        eprintln!("clip-arq-prontos: {dir} está fora da pasta de passagem");
        return std::process::ExitCode::FAILURE;
    }
    entregar(porta, &d)
}

/// Entrega os caminhos ao daemon local, que é quem mexe no clipboard.
///
/// Pelo daemon e não direto, pela mesma razão de
/// [`crate::clip::cli_set`]: no Linux quem escreve a seleção tem de
/// CONTINUAR VIVO para responder `SelectionTransfer` na hora da
/// colagem. Um processo de CLI que escreve e sai deixa uma seleção sem
/// dono, e o Ctrl+V não acha nada.
fn entregar(porta: u16, dir: &Path) -> std::process::ExitCode {
    let itens = entradas(dir);
    if itens.is_empty() {
        eprintln!("clip-arq: nada chegou em {}", dir.display());
        return std::process::ExitCode::FAILURE;
    }
    let corpo = serde_json::json!({ "paths": itens }).to_string().into_bytes();
    match postar(porta, "/api/clipboard-arquivos", &corpo) {
        Ok(()) => {
            println!("{} arquivo(s) no clipboard", itens.len());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("clip-arq: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// POST cru em localhost. Doze linhas contra um cliente HTTP inteiro.
fn postar(porta: u16, rota: &str, corpo: &[u8]) -> Result<(), String> {
    use std::io::{Read, Write};
    let pedido = format!(
        "POST {rota} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        corpo.len()
    );
    let mut s = std::net::TcpStream::connect(("127.0.0.1", porta))
        .map_err(|e| format!("o logid local não respondeu na porta {porta}: {e}"))?;
    s.write_all(pedido.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(corpo).map_err(|e| e.to_string())?;
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    if resp.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(resp.lines().next().unwrap_or("sem resposta").to_string())
    }
}

/// Escreve no clipboard local o que chegou de um par.
pub fn aplicar_local(caminhos: &[String]) -> Result<(), String> {
    if !registrar(caminhos) {
        return Ok(()); // já é esse; escrever de novo só geraria eco
    }
    #[cfg(target_os = "linux")]
    {
        crate::clip::pedir_arquivos(caminhos)
    }
    #[cfg(not(target_os = "linux"))]
    {
        escrever_local(caminhos)
    }
}
