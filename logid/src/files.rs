//! Navegacao de arquivos: local por `std::fs`, remoto por SFTP.
//!
//! Spec: ../specs/003-files.md
//!
//! ## Por que SFTP e nao um subcomando no agente
//!
//! O canal SSH para o par ja existe e ja e confiado -- e por ele que o
//! daemon sonda o agente e dispara o dead-man's switch. SFTP anda nesse
//! mesmo canal, com a mesma chave e a mesma autorizacao, e o
//! `sftp-server` existe no OpenSSH dos dois SOs. Um subcomando novo no
//! agente significaria mais superficie para manter e nada a mais em
//! troca.
//!
//! ## Copia sim, apagar nao
//!
//! O daemon e superficie de privilegio (spec 008), e escrever por acidente
//! num painel que eu mesmo sirvo e o tipo de erro que nao tem desfazer. Por
//! isso a escrita e estreita: so copia, entre esta maquina e outra, e nunca
//! por cima de algo que ja existe. Apagar e mover continuam de fora.
//!
//! ## Caminhos nao sao comparaveis entre as duas origens
//!
//! O SFTP expoe o disco do Windows como `/C:/Users/user`; o `std::fs`
//! na propria maquina diz `C:\Users\user`. Sao dois nomes para o mesmo
//! lugar, e nenhum dos dois esta errado. Em vez de normalizar (e errar),
//! cada listagem declara sua `source`, devolve `path` opaco para a UI
//! usar na navegacao, e um `display` separado para mostrar a gente.

use crate::config::Config;
use serde::Serialize;
use std::sync::Mutex;
use std::io::Write;
use std::process::Stdio;

/// Teto de entradas por listagem. Uma pasta com 50 mil arquivos nao
/// ajuda ninguem na tela e enche o JSON.
const MAX_ENTRIES: usize = 2000;
const SFTP_TIMEOUT_SECS: u64 = 15;

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    /// dir | file | link | other
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Epoca unix, quando a origem sabe dizer (caminho local).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<u64>,
    /// Texto como a origem entregou, quando nao da epoca (SFTP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub host: String,
    /// Caminho canonico para navegar -- opaco, devolver como veio.
    pub path: String,
    /// Como mostrar na tela.
    pub display: String,
    /// local | sftp
    pub source: &'static str,
    /// Caminho do pai, ou `None` na raiz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    /// Passou de `MAX_ENTRIES` e foi cortada.
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Listing {
    fn failed(host: &str, path: &str, source: &'static str, msg: impl Into<String>) -> Self {
        Listing {
            host: host.into(),
            path: path.into(),
            display: path.into(),
            source,
            parent: None,
            entries: vec![],
            truncated: false,
            error: Some(msg.into()),
        }
    }
}

/// Decodifica percent-encoding da query. Sem crate: e um parser de uma
/// tabela so, e o `path` chega com `/`, `\`, `:` e espaco.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lista `path` em `host`. `path` vazio = ponto de partida daquele host.
pub fn list(cfg: &Config, host: &str, path: &str) -> Listing {
    let Some(h) = cfg.hosts.iter().find(|x| x.name == host) else {
        return Listing::failed(host, path, "local", format!("host '{host}' nao esta na config"));
    };
    if h.is_local() {
        return list_local(host, path);
    }
    let Some(alias) = h.ssh.as_deref() else {
        return Listing::failed(
            host,
            path,
            "sftp",
            format!(
                "host '{host}' nao tem `ssh` na config: sem canal SSH nao ha \
                 como listar. No Ubuntu o `sshd` ainda nao esta instalado -- \
                 ver specs/003."
            ),
        );
    };
    list_sftp(host, alias, &h.addr, path)
}

// --- local -----------------------------------------------------------------

fn list_local(host: &str, path: &str) -> Listing {
    let p = if path.is_empty() {
        crate::sys::home_dir()
    } else {
        std::path::PathBuf::from(path)
    };
    let dir = match std::fs::read_dir(&p) {
        Ok(d) => d,
        Err(e) => {
            return Listing::failed(host, &p.to_string_lossy(), "local", format!("{e}"));
        }
    };

    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;
    for e in dir.flatten() {
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        // `metadata()` segue link; `file_type()` nao. Queremos saber que
        // E link, e ainda assim poder dizer se aponta para pasta.
        let ft = e.file_type().ok();
        let md = e.metadata().ok();
        let kind = match (&ft, &md) {
            (Some(t), _) if t.is_symlink() => "link",
            (_, Some(m)) if m.is_dir() => "dir",
            (_, Some(m)) if m.is_file() => "file",
            _ => "other",
        };
        let size = md.as_ref().filter(|m| m.is_file()).map(|m| m.len());
        let mtime = md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        entries.push(Entry { name, kind, size, mtime, mtime_text: None });
    }
    sort_entries(&mut entries);

    let canon = p.to_string_lossy().into_owned();
    Listing {
        host: host.into(),
        path: canon.clone(),
        display: canon,
        source: "local",
        parent: p.parent().map(|x| x.to_string_lossy().into_owned()),
        entries,
        truncated,
        error: None,
    }
}

// --- remoto por SFTP -------------------------------------------------------

/// Roda um lote no `sftp` e devolve o stdout.
///
/// `-b -` le os comandos do stdin. Lote em vez de sessao interativa
/// porque o daemon nao tem com quem conversar: um pedido, uma resposta.
fn sftp_batch(alias: &str, addr: &str, script: &str) -> Result<String, String> {
    let mut args: Vec<String> = vec![
        "-q".into(),
        "-b".into(),
        "-".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={SFTP_TIMEOUT_SECS}"),
        "-o".into(),
        "LogLevel=ERROR".into(),
    ];
    // Mesmo contorno do `agent.rs`: resolvemos aqui, porque o resolvedor
    // do ssh/sftp lancado pelo daemon no Windows nao enxerga o nome.
    // Ver `probe::ssh_addr_opts`.
    args.extend(crate::probe::ssh_addr_opts(addr));
    args.push(alias.to_string());
    let mut child = crate::sys::child("sftp")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("nao consegui executar sftp: {e}"))?;

    child
        .stdin
        .as_mut()
        .ok_or("sftp sem stdin")?
        .write_all(script.as_bytes())
        .map_err(|e| format!("escrever no sftp: {e}"))?;
    drop(child.stdin.take());

    let out = child.wait_with_output().map_err(|e| format!("esperar sftp: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "sftp falhou sem mensagem".into() } else { err });
    }
    Ok(stdout)
}

fn list_sftp(host: &str, alias: &str, addr: &str, path: &str) -> Listing {
    // UM lote, nao dois. A primeira versao chamava `pwd` e depois
    // `cd`+`ls`, pagando dois handshakes SSH -- 809 ms medidos para
    // listar uma pasta. O `pwd` no mesmo lote da o caminho canonico de
    // graca, e e dele que saem o `display` e o `parent`: assim a UI
    // navega pelo que o servidor confirma, nao pelo que eu supus.
    //
    // Aspas simples no `cd`: nomes com espaco sao comuns (`C:/Program
    // Files`). As aspas da entrada sao removidas em vez de escapadas --
    // nome de arquivo com aspas simples e raro o bastante para nao valer
    // a complexidade, e o pior caso e a listagem falhar, nao vazar
    // comando.
    let script = if path.is_empty() {
        "pwd\nls -la\n".to_string()
    } else {
        format!("cd '{}'\npwd\nls -la\n", path.replace('\'', ""))
    };
    let out = match sftp_batch(alias, addr, &script) {
        Ok(o) => o,
        Err(e) => return Listing::failed(host, path, "sftp", e),
    };
    let Some(canon) = parse_pwd(&out) else {
        return Listing::failed(
            host,
            path,
            "sftp",
            if path.is_empty() {
                "sftp nao respondeu ao pwd".to_string()
            } else {
                format!("nao consegui entrar em '{path}'")
            },
        );
    };

    let (mut entries, truncated) = parse_ls_la(&out);
    sort_entries(&mut entries);

    Listing {
        host: host.into(),
        path: canon.clone(),
        display: windows_display(&canon),
        source: "sftp",
        parent: sftp_parent(&canon),
        entries,
        truncated,
        error: None,
    }
}

fn parse_pwd(out: &str) -> Option<String> {
    // "Remote working directory: /C:/Users/user"
    out.lines()
        .find_map(|l| l.split_once("working directory:").map(|(_, p)| p.trim().to_string()))
        .filter(|p| !p.is_empty())
}

/// Pai de um caminho SFTP. `/C:/` e a raiz do disco: acima dela o
/// `sftp` nao lista nada util, entao paramos ali.
fn sftp_parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let idx = trimmed.rfind('/')?;
    if idx == 0 {
        // "/C:" -> nao subimos para "/"
        return None;
    }
    Some(trimmed[..idx].to_string())
}

/// `/C:/Users/user` -> `C:\Users\user`. Para os olhos, nao para
/// navegar: quem navega usa o `path`.
fn windows_display(path: &str) -> String {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[2] == b':' && (b[1] as char).is_ascii_alphabetic() {
        return path[1..].replace('/', "\\");
    }
    path.to_string()
}

/// Parseia a saida de `ls -la` do sftp.
///
/// Formato (o do servidor, nao o nosso):
///   `drwxrwxrwx   1 user user  0 Nov  3 22:00 Documents`
///
/// **Ancorado no mes, nao em contagem de campos.** A primeira versao
/// contava: nome = depois do 8o campo. Errado -- sao nove (modo, links,
/// dono, grupo, tamanho, mes, dia, hora, nome), e o teste pegou: `.` e
/// `..` passavam pelo filtro porque o nome lido era outro pedaco da
/// linha. Corrigir o indice para 9 nao bastaria: ha servidor que omite o
/// grupo, e dai sao oito. Procurar o mes acha a juncao em qualquer um dos
/// dois casos, porque dali para a direita o formato e fixo:
/// `<mes> <dia> <hora-ou-ano> <nome>`, e o tamanho e o campo imediatamente
/// anterior.
fn parse_ls_la(out: &str) -> (Vec<Entry>, bool) {
    const MESES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut entries = Vec::new();
    let mut truncated = false;
    for line in out.lines() {
        let line = line.trim_end();
        // Linhas de eco do proprio sftp e totais do ls.
        if line.starts_with("sftp>") || line.starts_with("total ") || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let mode = fields[0];
        if mode.len() < 10 || !matches!(mode.as_bytes()[0], b'd' | b'-' | b'l') {
            continue;
        }
        // O mes nao pode ser o campo 0 nem 1; comecar em 2 evita casar
        // com um dono chamado "Mar".
        let Some(m) = (2..fields.len().saturating_sub(2))
            .find(|&i| MESES.contains(&fields[i]) && fields[i + 1].parse::<u8>().is_ok())
        else {
            continue;
        };
        let size = fields[m - 1].parse::<u64>().ok();
        let when = format!("{} {} {}", fields[m], fields[m + 1], fields[m + 2]);

        // O nome e tudo depois do campo da hora -- posicional de
        // proposito, porque nome com espaco e comum (`Program Files`) e
        // juntar os campos restantes colapsaria espacos repetidos.
        let name = match nth_field_offset(line, m + 4) {
            Some(i) => line[i..].to_string(),
            None => continue,
        };
        if name == "." || name == ".." {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let kind = match mode.as_bytes()[0] {
            b'd' => "dir",
            b'l' => "link",
            b'-' => "file",
            _ => "other",
        };
        entries.push(Entry {
            name,
            kind,
            size: if kind == "file" { size } else { None },
            mtime: None,
            mtime_text: Some(when),
        });
    }
    (entries, truncated)
}

/// Byte onde comeca o n-esimo campo (1-based) separado por espacos.
fn nth_field_offset(line: &str, n: usize) -> Option<usize> {
    let b = line.as_bytes();
    let mut i = 0;
    let mut field = 0;
    while i < b.len() {
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        field += 1;
        if field == n {
            return Some(i);
        }
        while i < b.len() && b[i] != b' ' {
            i += 1;
        }
    }
    None
}

/// Pastas antes de arquivos, e dentro de cada grupo por nome sem
/// diferenciar caixa -- a ordem que a pessoa espera, nao a do SO.
fn sort_entries(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        let rank = |k: &str| match k {
            "dir" => 0,
            "link" => 1,
            _ => 2,
        };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

// --- transferencia ---------------------------------------------------------

/// Uma copia pedida pela tela. O `estado` e `andando`, `ok` ou `erro`.
#[derive(Debug, Clone, Serialize)]
pub struct Transferencia {
    pub id: u64,
    pub nome: String,
    pub de: String,
    pub para: String,
    pub estado: &'static str,
    pub detalhe: String,
}

static TRANSFERENCIAS: Mutex<Vec<Transferencia>> = Mutex::new(Vec::new());
/// Quantas ficam na lista da tela. A copia em si nao depende disso.
const MAX_TRANSFERENCIAS: usize = 30;

pub fn transferencias() -> Vec<Transferencia> {
    TRANSFERENCIAS.lock().map(|g| g.clone()).unwrap_or_default()
}

fn concluir(id: u64, estado: &'static str, detalhe: String) {
    if let Ok(mut g) = TRANSFERENCIAS.lock() {
        if let Some(t) = g.iter_mut().find(|t| t.id == id) {
            t.estado = estado;
            t.detalhe = detalhe;
        }
    }
}

/// Recusa o que nao cabe entre aspas no lote do sftp, em vez de alterar o
/// nome em silencio e copiar outra coisa. `*?[` seriam glob no `get`.
fn caminho_seguro(c: &str) -> Result<(), String> {
    match c.chars().find(|x| matches!(x, '\'' | '"' | '\n' | '\r' | '*' | '?' | '[')) {
        Some(x) => Err(format!("o caminho tem um caractere que nao sei copiar: {x:?}")),
        None => Ok(()),
    }
}

/// Junta pasta e nome com `/`: o sftp usa isso nos dois sistemas.
fn juntar(pasta: &str, nome: &str) -> String {
    format!("{}/{nome}", pasta.trim_end_matches(['/', '\\']))
}

fn nome_de(caminho: &str) -> Option<&str> {
    caminho.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|n| !n.is_empty())
}

fn tamanho_em(cfg: &Config, host: &str, pasta: &str, nome: &str) -> Result<Option<Entry>, String> {
    let l = list(cfg, host, pasta);
    if let Some(e) = l.error {
        return Err(e);
    }
    Ok(l.entries.into_iter().find(|e| e.name == nome))
}

/// Copia `caminho` de `de` para a pasta `pasta` de `para`. Devolve o id, e a
/// copia segue numa thread: um arquivo grande nao pode prender o servidor.
pub fn transferir(
    cfg: &Config,
    de: &str,
    caminho: &str,
    para: &str,
    pasta: &str,
) -> Result<u64, String> {
    let achar = |n: &str| {
        cfg.hosts.iter().find(|h| h.name == n).ok_or_else(|| format!("host '{n}' nao esta na config"))
    };
    let (ho, hd) = (achar(de)?, achar(para)?);
    let remoto = match (ho.is_local(), hd.is_local()) {
        (true, false) => hd,
        (false, true) => ho,
        _ => return Err("uma das pontas tem de ser esta maquina".into()),
    };
    let alias = remoto.ssh.clone().ok_or_else(|| format!("'{}' nao tem `ssh`", remoto.name))?;
    caminho_seguro(caminho)?;
    caminho_seguro(pasta)?;
    let nome = nome_de(caminho).ok_or("caminho sem nome")?.to_string();
    let pai = caminho.trim_end_matches(['/', '\\']);
    let pai = pai[..pai.len() - nome.len()].to_string();

    let origem = tamanho_em(cfg, de, &pai, &nome)?.ok_or("o arquivo nao esta mais la")?;
    if tamanho_em(cfg, para, pasta, &nome)?.is_some() {
        return Err(format!("'{nome}' ja existe no destino: nada foi copiado"));
    }

    let destino = juntar(pasta, &nome);
    let comando = if ho.is_local() { "put" } else { "get" };
    let script = format!("{comando} -r '{caminho}' '{destino}'\n");
    let mut g = TRANSFERENCIAS.lock().map_err(|_| "lista travada")?;
    let id = g.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    g.push(Transferencia {
        id,
        nome: nome.clone(),
        de: de.into(),
        para: para.into(),
        estado: "andando",
        detalhe: String::new(),
    });
    let sobra = g.len().saturating_sub(MAX_TRANSFERENCIAS);
    g.drain(..sobra);
    drop(g);

    let (cfg, para, pasta, addr) =
        (cfg.clone(), para.to_string(), pasta.to_string(), remoto.addr.clone());
    std::thread::spawn(move || {
        if let Err(e) = sftp_batch(&alias, &addr, &script) {
            return concluir(id, "erro", e);
        }
        // Terminar nao e ter chegado: quem responde e o destino.
        match tamanho_em(&cfg, &para, &pasta, &nome) {
            Ok(Some(e)) if origem.kind != "file" || e.size == origem.size => {
                let t = e.size.map(|b| format!("{b} bytes")).unwrap_or_else(|| "pasta".into());
                say!("files: {nome} copiado para {para}");
                concluir(id, "ok", t);
            }
            Ok(Some(e)) => concluir(
                id,
                "erro",
                format!("chegou com {:?} bytes, esperava {:?}", e.size, origem.size),
            ),
            Ok(None) => concluir(id, "erro", "o destino nao tem o arquivo".into()),
            Err(e) => concluir(id, "erro", e),
        }
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caminho_com_aspas_ou_glob_e_recusado() {
        assert!(caminho_seguro("/C:/Users/user/Meus Arquivos/a.txt").is_ok());
        assert!(caminho_seguro("/tmp/a'b").is_err());
        assert!(caminho_seguro("/tmp/*").is_err());
        assert!(caminho_seguro("/tmp/a\nb").is_err());
    }

    #[test]
    fn nome_sai_do_fim_do_caminho() {
        assert_eq!(nome_de("/C:/Users/a.txt"), Some("a.txt"));
        assert_eq!(nome_de("C:\\Users\\pasta\\"), Some("pasta"));
        assert_eq!(nome_de("/"), None);
        assert_eq!(juntar("/tmp/", "a"), "/tmp/a");
    }


    #[test]
    fn decodifica_percent_e_mais() {
        assert_eq!(percent_decode("%2FC%3A%2FProgram%20Files"), "/C:/Program Files");
        assert_eq!(percent_decode("a+b"), "a b");
        // `%` solto nao pode derrubar o parser.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn parseia_ls_la_do_sftp() {
        let out = "\
sftp> cd '/C:/Users/user'
sftp> ls -la
drwxrwxrwx    1 user user        0 Nov  3 22:00 .
drwxrwxrwx    1 user user        0 Oct 26 14:05 ..
drwxrwxrwx    1 user user        0 Nov  3 21:12 Program Files
-rw-rw-rw-    1 user user     1234 Nov  3 22:19 watch-edge.log
lrwxrwxrwx    1 user user       12 Nov  1 09:00 atalho
total 4
";
        let (mut v, truncated) = parse_ls_la(out);
        sort_entries(&mut v);
        assert!(!truncated);
        // `.` e `..` saem; o pai vem do caminho, nao da listagem.
        assert_eq!(v.len(), 3);
        // Pastas primeiro, depois link, depois arquivo.
        assert_eq!(v[0].name, "Program Files", "nome com espaco tem de sobreviver");
        assert_eq!(v[0].kind, "dir");
        assert_eq!(v[1].kind, "link");
        assert_eq!(v[2].name, "watch-edge.log");
        assert_eq!(v[2].size, Some(1234));
        assert_eq!(v[2].mtime_text.as_deref(), Some("Nov 3 22:19"));
    }

    #[test]
    fn parseia_mesmo_sem_o_campo_de_grupo() {
        // Ha servidor SFTP que omite o grupo: oito campos em vez de nove.
        // Contagem fixa quebraria aqui; a ancora do mes nao.
        let out = "-rw-r--r--    1 user    4096 Nov  3 22:00 nota final.txt\n";
        let (v, _) = parse_ls_la(out);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "nota final.txt");
        assert_eq!(v[0].size, Some(4096));
    }

    #[test]
    fn ano_em_vez_de_hora_para_arquivo_antigo() {
        // `ls` troca a hora pelo ano quando o arquivo tem mais de 6 meses.
        let out = "-rw-r--r--    1 v v    10 Jan  2  2025 velho.txt\n";
        let (v, _) = parse_ls_la(out);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "velho.txt");
        assert_eq!(v[0].mtime_text.as_deref(), Some("Jan 2 2025"));
    }

    #[test]
    fn caminho_windows_vira_legivel_mas_navegavel() {
        assert_eq!(windows_display("/C:/Users/user"), "C:\\Users\\user");
        // Caminho unix fica como esta.
        assert_eq!(windows_display("/home/user"), "/home/user");
    }

    #[test]
    fn pai_para_na_raiz_do_disco() {
        assert_eq!(sftp_parent("/C:/Users/user"), Some("/C:/Users".into()));
        assert_eq!(sftp_parent("/C:/Users"), Some("/C:".into()));
        // Acima de `/C:` o sftp nao lista nada util.
        assert_eq!(sftp_parent("/C:"), None);
        assert_eq!(sftp_parent("/"), None);
    }

    #[test]
    fn pwd_do_sftp() {
        assert_eq!(
            parse_pwd("Remote working directory: /C:/Users/user\n").as_deref(),
            Some("/C:/Users/user")
        );
        assert_eq!(parse_pwd("nada aqui"), None);
    }
}
