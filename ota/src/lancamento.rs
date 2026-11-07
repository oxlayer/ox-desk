//! O que a Release do GitHub diz, e o que fazer com isso.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Tudo aqui e funcao pura sobre texto: o JSON que a API devolveu, o
//! `SHA256SUMS` que veio com a Release, o nome do pacote. A rede, o
//! disco e os comandos de cada sistema ficam em `aplicar.rs` -- a
//! decisao "isto e mais novo e vale trocar?" tem de ser testavel sem
//! nenhum GitHub por perto.

use crate::plano::Sistema;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ativo {
    pub nome: String,
    /// O endereco da API (`.../releases/assets/<id>`), nao o
    /// `browser_download_url`.
    ///
    /// Em repo privado o `browser_download_url` responde **404** mesmo
    /// com token: ele serve o CDN, que nao conhece a autenticacao. O
    /// endereco da API serve o arquivo quando se pede
    /// `Accept: application/octet-stream`.
    pub url: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lancamento {
    pub tag: String,
    pub ativos: Vec<Ativo>,
}

/// Le a resposta de `/releases/latest`.
pub fn ler(json: &str) -> Result<Lancamento, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("a resposta nao e JSON: {e}"))?;

    // A API responde 200 com `{"message": "..."}` em varios casos
    // (sem token em repo privado, limite de chamadas estourado). Sem
    // isto o erro apareceria como "nenhum pacote para linux", que
    // manda procurar no lugar errado.
    if v.get("assets").is_none() {
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("sem assets e sem message");
        return Err(format!("o GitHub respondeu: {msg}"));
    }

    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("?").to_string();
    let ativos = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Ativo {
                        nome: x.get("name")?.as_str()?.to_string(),
                        url: x.get("url")?.as_str()?.to_string(),
                        bytes: x.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Lancamento { tag, ativos })
}

/// O pacote deste sistema, pelo prefixo que o `construir-binarios.sh` usa.
pub fn escolher<'a>(l: &'a Lancamento, s: Sistema) -> Result<&'a Ativo, String> {
    let p = crate::plano::prefixo(s);
    l.ativos
        .iter()
        .find(|a| a.nome.starts_with(p) && a.nome.ends_with(".tar.gz"))
        .ok_or_else(|| {
            let tem: Vec<&str> = l.ativos.iter().map(|a| a.nome.as_str()).collect();
            format!("nenhum pacote comecando com {p}; a Release traz: {}", tem.join(", "))
        })
}

/// O `SHA256SUMS` da Release.
pub fn somas<'a>(l: &'a Lancamento) -> Result<&'a Ativo, String> {
    l.ativos
        .iter()
        .find(|a| a.nome == "SHA256SUMS")
        .ok_or_else(|| "a Release nao traz SHA256SUMS; nao confiro o que baixei".to_string())
}

/// O `-<n>-<hash>` que o nome do pacote carrega.
///
/// `logi-linux-x86_64-124-abc1234.tar.gz` -> `(124, "abc1234")`. A regra
/// e "os dois ultimos campos antes do `.tar.gz`", o que vale tambem para
/// `logi-windows-x86_64-unsigned-...`, que tem um campo a mais no meio.
pub fn carimbo_do_nome(nome: &str) -> Option<(u64, String)> {
    let base = nome.strip_suffix(".tar.gz")?;
    let mut campos = base.rsplit('-');
    let hash = campos.next()?;
    let n: u64 = campos.next()?.parse().ok()?;
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((n, hash.to_string()))
}

/// A soma esperada de um arquivo, lida de um `SHA256SUMS`.
///
/// Formato do `sha256sum`/`shasum`: `<64 hex><dois espacos><nome>`. O
/// nome pode vir com caminho (`dist/x.tar.gz`) porque quem gerou pode
/// ter rodado de outro diretorio; compara-se o nome base.
pub fn soma_de(sums: &str, nome: &str) -> Option<String> {
    linhas(sums).into_iter().find(|(_, n)| n == nome).map(|(s, _)| s)
}

/// Todas as linhas de um `SHA256SUMS`, como `(soma, nome base)`.
pub fn linhas(sums: &str) -> Vec<(String, String)> {
    sums.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (soma, resto) = l.split_once(' ')?;
            if soma.len() != 64 || !soma.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            // `*` do modo binario e a barra de caminho saem fora.
            let nome = resto.trim().trim_start_matches('*');
            let nome = nome.rsplit(['/', '\\']).next()?;
            Some((soma.to_ascii_lowercase(), nome.to_string()))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decisao {
    /// O que esta na Release ja esta instalado.
    Igual { n: u64, hash: String },
    /// Daqui saiu build mais novo que o publicado -- um deploy local.
    /// Nao se desce de versao sozinho.
    MaisNovoAqui { aqui: u64, la: u64 },
    Atualizar { n: u64, hash: String },
    /// O nome do pacote nao traz `-<n>-<hash>`: sem isso nao se compara
    /// nada, e trocar binario as cegas e pior que nao trocar.
    SemCarimbo { nome: String },
}

/// Vale trocar?
///
/// Compara pelo `n` (`git rev-list --count`), que cresce sempre, e nao
/// pela tag: a tag e escolhida a mao e ja apareceu fora de ordem em
/// qualquer projeto que use tags. O hash entra para o caso de `n` igual
/// com commit diferente -- que e o que um rebase produz.
pub fn decidir(n_aqui: u64, hash_aqui: &str, nome_do_pacote: &str) -> Decisao {
    let Some((n_la, hash_la)) = carimbo_do_nome(nome_do_pacote) else {
        return Decisao::SemCarimbo { nome: nome_do_pacote.to_string() };
    };
    // `abc1234-sujo` e o carimbo de um deploy com arvore suja: conta
    // como o mesmo commit para efeito de comparacao, e a diferenca de
    // `n` e que decide.
    let aqui_limpo = hash_aqui.strip_suffix("-sujo").unwrap_or(hash_aqui);
    if n_la == n_aqui && aqui_limpo == hash_la {
        return Decisao::Igual { n: n_la, hash: hash_la };
    }
    if n_la < n_aqui {
        return Decisao::MaisNovoAqui { aqui: n_aqui, la: n_la };
    }
    Decisao::Atualizar { n: n_la, hash: hash_la }
}

/// O `n` e o `build` que o `/healthz` do daemon vivo devolve.
pub fn ler_healthz(json: &str) -> Option<(u64, String)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let n = v.get("n")?.as_u64()?;
    let b = v.get("build")?.as_str()?.to_string();
    Some((n, b))
}

#[cfg(test)]
mod testes {
    use super::*;

    const RELEASE: &str = r#"{
      "tag_name": "v0.1.0",
      "assets": [
        {"name":"SHA256SUMS","url":"https://api.github.com/x/releases/assets/1","size":300},
        {"name":"logi-linux-x86_64-124-abc1234.tar.gz","url":"https://api.github.com/x/releases/assets/2","size":9000000},
        {"name":"logi-mac-arm64-unsigned-124-abc1234.tar.gz","url":"https://api.github.com/x/releases/assets/3","size":8000000},
        {"name":"logi-windows-x86_64-unsigned-124-abc1234.tar.gz","url":"https://api.github.com/x/releases/assets/4","size":7000000}
      ]
    }"#;

    #[test]
    fn le_a_release_e_acha_o_pacote_de_cada_sistema() {
        let l = ler(RELEASE).unwrap();
        assert_eq!(l.tag, "v0.1.0");
        assert_eq!(l.ativos.len(), 4);
        assert_eq!(escolher(&l, Sistema::Linux).unwrap().nome, "logi-linux-x86_64-124-abc1234.tar.gz");
        assert_eq!(escolher(&l, Sistema::Mac).unwrap().nome, "logi-mac-arm64-unsigned-124-abc1234.tar.gz");
        assert_eq!(
            escolher(&l, Sistema::Windows).unwrap().nome,
            "logi-windows-x86_64-unsigned-124-abc1234.tar.gz"
        );
        assert_eq!(somas(&l).unwrap().bytes, 300);
    }

    /// O prefixo do Linux e prefixo de mais ninguem. Se um dia alguem
    /// renomear os pacotes, este teste cai antes de a maquina baixar o
    /// binario do sistema errado.
    #[test]
    fn o_prefixo_de_um_sistema_nao_casa_com_outro() {
        let l = ler(RELEASE).unwrap();
        for s in [Sistema::Linux, Sistema::Mac, Sistema::Windows] {
            let escolhido = escolher(&l, s).unwrap();
            let outros: Vec<&Ativo> = l
                .ativos
                .iter()
                .filter(|a| a.nome.ends_with(".tar.gz") && a.nome != escolhido.nome)
                .collect();
            for o in outros {
                assert!(
                    !o.nome.starts_with(crate::plano::prefixo(s)),
                    "{:?}: {} tambem casa", s, o.nome
                );
            }
        }
    }

    /// A API responde 200 com `{"message":...}` quando falta token em
    /// repo privado. Confundir isso com "nao ha pacote" mandaria
    /// procurar o problema na CI.
    #[test]
    fn mensagem_de_erro_da_api_nao_vira_pacote_ausente() {
        let e = ler(r#"{"message":"Not Found","documentation_url":"..."}"#).unwrap_err();
        assert!(e.contains("Not Found"), "{e}");
    }

    #[test]
    fn carimbo_sai_do_nome_com_e_sem_campo_extra() {
        assert_eq!(carimbo_do_nome("logi-linux-x86_64-124-abc1234.tar.gz"), Some((124, "abc1234".into())));
        assert_eq!(
            carimbo_do_nome("logi-windows-x86_64-unsigned-124-abc1234.tar.gz"),
            Some((124, "abc1234".into()))
        );
        assert_eq!(carimbo_do_nome("logi-linux-x86_64.tar.gz"), None);
        assert_eq!(carimbo_do_nome("SHA256SUMS"), None);
        // `zzzzzzz` nao e hexadecimal: nao e hash de commit.
        assert_eq!(carimbo_do_nome("logi-linux-x86_64-124-zzzzzzz.tar.gz"), None);
    }

    #[test]
    fn decide_pelo_n_e_desempata_pelo_hash() {
        let novo = "logi-linux-x86_64-124-abc1234.tar.gz";
        assert_eq!(decidir(124, "abc1234", novo), Decisao::Igual { n: 124, hash: "abc1234".into() });
        assert_eq!(decidir(120, "0000000", novo), Decisao::Atualizar { n: 124, hash: "abc1234".into() });
        assert_eq!(decidir(130, "0000000", novo), Decisao::MaisNovoAqui { aqui: 130, la: 124 });
        // Mesmo `n`, commit diferente: um rebase reescreveu a historia.
        // Trocar e o certo -- o que esta publicado e a verdade.
        assert_eq!(decidir(124, "0000000", novo), Decisao::Atualizar { n: 124, hash: "abc1234".into() });
    }

    /// `-sujo` e como o `deploy.sh` carimba build de arvore suja. Sem
    /// este caso, uma maquina acabada de receber deploy local baixaria o
    /// mesmo commit da Release por "hash diferente", em laco.
    #[test]
    fn carimbo_sujo_conta_como_o_mesmo_commit() {
        assert_eq!(
            decidir(124, "abc1234-sujo", "logi-linux-x86_64-124-abc1234.tar.gz"),
            Decisao::Igual { n: 124, hash: "abc1234".into() }
        );
    }

    /// Nunca trocar sem saber o que se esta trocando.
    #[test]
    fn sem_carimbo_nao_atualiza() {
        assert!(matches!(decidir(1, "x", "logi-linux-x86_64.tar.gz"), Decisao::SemCarimbo { .. }));
    }

    #[test]
    fn le_somas_com_caminho_e_modo_binario() {
        let sums = "\
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  dist/logi-linux-x86_64-124-abc1234.tar.gz
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad *logid
lixo que nao e soma
";
        assert_eq!(
            soma_de(sums, "logi-linux-x86_64-124-abc1234.tar.gz").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(soma_de(sums, "logid").unwrap().len(), 64);
        assert_eq!(soma_de(sums, "nao-existe"), None);
        assert_eq!(linhas(sums).len(), 2, "a linha de lixo nao conta");
    }

    #[test]
    fn le_o_healthz() {
        let j = r#"{"ok":true,"build":"abc1234","n":124,"host":"x"}"#;
        assert_eq!(ler_healthz(j), Some((124, "abc1234".into())));
        assert_eq!(ler_healthz("nao e json"), None);
        assert_eq!(ler_healthz(r#"{"ok":true}"#), None);
    }
}
