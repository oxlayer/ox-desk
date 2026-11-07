//! O que ficou da ultima checagem, em disco.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Existe porque o OTA roda sozinho, por tarefa agendada, e ninguem
//! esta olhando quando ele roda. Sem este arquivo a unica forma de
//! saber se a maquina esta se atualizando seria ler log -- e numero que
//! ninguem imprime, ninguem confere
//! ([[../../.claude/rules/verificar-o-que-chegou]]). O passo `ota` do
//! wizard le daqui.
//!
//! So `quando_unix`, sem data formatada: o que o painel mostra e "ha
//! quanto tempo", e formatar hora local sem dependencia nova e o tipo de
//! codigo que erra em fuso e em horario de verao.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estado {
    pub quando_unix: u64,
    pub sistema: String,
    /// O que esta instalado nesta maquina, como o `/healthz` responde.
    pub n_aqui: u64,
    pub build_aqui: String,
    /// O que a Release publicou. Zerado quando nem chegamos a saber.
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub n_la: u64,
    #[serde(default)]
    pub build_la: String,
    #[serde(default)]
    pub pacote: String,
    /// `igual` | `atualizado` | `novidade` | `mais-novo-aqui` |
    /// `revertido` | `erro`
    pub resultado: String,
    pub mensagem: String,
}

impl Estado {
    pub fn novo(sistema: &str, n_aqui: u64, build_aqui: &str) -> Estado {
        Estado {
            quando_unix: agora(),
            sistema: sistema.to_string(),
            n_aqui,
            build_aqui: build_aqui.to_string(),
            tag: String::new(),
            n_la: 0,
            build_la: String::new(),
            pacote: String::new(),
            resultado: "erro".into(),
            mensagem: "sem conclusao".into(),
        }
    }

    /// Resumo de uma linha, do jeito que o wizard mostra.
    pub fn linha(&self) -> String {
        match self.resultado.as_str() {
            "igual" => format!("em dia no build #{} {}", self.n_aqui, self.build_aqui),
            "atualizado" => {
                format!("atualizei para #{} {} sozinho", self.n_la, self.build_la)
            }
            "novidade" => format!(
                "ha o build #{} {} publicado; aqui esta o #{} -- {}",
                self.n_la, self.build_la, self.n_aqui, self.mensagem
            ),
            "mais-novo-aqui" => format!(
                "aqui esta o #{}, mais novo que o #{} publicado",
                self.n_aqui, self.n_la
            ),
            "revertido" => format!("voltei para #{} {}: {}", self.n_aqui, self.build_aqui, self.mensagem),
            _ => self.mensagem.clone(),
        }
    }
}

/// Uma linha: `<resultado> TAB <idade em segundos> TAB <frase>`.
///
/// O contrato que o passo `ota` do wizard consome. TAB porque a frase
/// tem `;`, `:` e virgula, e nenhuma delas serve de separador; TAB nao
/// aparece em nada que `linha()` produz, e o teste prende isso.
pub fn curto(e: &Estado, agora: u64) -> String {
    let idade = agora.saturating_sub(e.quando_unix);
    format!("{}\t{}\t{}", e.resultado, idade, e.linha().replace(['\t', '\n'], " "))
}

pub fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn caminho(dir: &Path) -> PathBuf {
    dir.join("estado.json")
}

pub fn escrever(dir: &Path, e: &Estado) -> Result<(), String> {
    let txt = serde_json::to_string_pretty(e).map_err(|x| x.to_string())?;
    std::fs::write(caminho(dir), txt).map_err(|x| format!("{}: {x}", caminho(dir).display()))
}

/// Le o estado. Arquivo ausente ou corrompido devolve `None` -- nunca
/// derruba a proxima checagem.
pub fn ler(dir: &Path) -> Option<Estado> {
    let txt = std::fs::read_to_string(caminho(dir)).ok()?;
    serde_json::from_str(&txt).ok()
}

#[cfg(test)]
mod testes {
    use super::*;

    fn exemplo() -> Estado {
        let mut e = Estado::novo("mac", 124, "abc1234");
        e.tag = "v0.1.0".into();
        e.n_la = 125;
        e.build_la = "def5678".into();
        e.pacote = "logi-mac-arm64-unsigned-125-def5678.tar.gz".into();
        e.resultado = "atualizado".into();
        e.mensagem = "trocado e conferido".into();
        e
    }

    #[test]
    fn ida_e_volta_pelo_disco() {
        let dir = std::env::temp_dir().join(format!("logi-ota-teste-{}", agora()));
        std::fs::create_dir_all(&dir).unwrap();
        let e = exemplo();
        escrever(&dir, &e).unwrap();
        assert_eq!(ler(&dir).unwrap(), e);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lixo_no_arquivo_nao_derruba() {
        let dir = std::env::temp_dir().join(format!("logi-ota-lixo-{}", agora()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(caminho(&dir), "nao e json").unwrap();
        assert!(ler(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sem_arquivo_devolve_nada() {
        assert!(ler(Path::new("/tmp/nao-existe-logi-ota-xyz")).is_none());
    }

    /// Cada resultado tem de produzir uma frase que diga o que aconteceu.
    /// Linha vazia no painel e o mesmo que nao ter painel.
    #[test]
    fn todo_resultado_tem_frase() {
        for r in ["igual", "atualizado", "novidade", "mais-novo-aqui", "revertido", "erro"] {
            let mut e = exemplo();
            e.resultado = r.into();
            let l = e.linha();
            assert!(!l.trim().is_empty(), "{r} nao diz nada");
            assert!(l.len() > 8, "{r}: \"{l}\"");
        }
    }

    /// Tres campos, sempre, e TAB so entre eles -- e o que o wizard
    /// corta. Frase com TAB dentro quebraria o passo `ota` do painel
    /// sem quebrar nenhum teste daqui, entao o teste e aqui.
    #[test]
    fn o_curto_tem_tres_campos_e_nenhum_tab_dentro() {
        for r in ["igual", "atualizado", "novidade", "mais-novo-aqui", "revertido", "erro"] {
            let mut e = exemplo();
            e.resultado = r.into();
            e.quando_unix = 1000;
            let l = curto(&e, 1725);
            let campos: Vec<&str> = l.split('\t').collect();
            assert_eq!(campos.len(), 3, "{r}: {l:?}");
            assert_eq!(campos[0], r);
            assert_eq!(campos[1], "725", "a idade e agora menos quando");
            assert!(!campos[2].trim().is_empty());
        }
    }

    /// Relogio que andou para tras (ou estado do futuro) nao pode virar
    /// idade gigante por estouro.
    #[test]
    fn idade_nunca_e_negativa_nem_estoura() {
        let mut e = exemplo();
        e.quando_unix = 9999;
        assert!(curto(&e, 10).contains("\t0\t"));
    }

    /// O campo novo que um dia entrar nao pode invalidar o arquivo que
    /// a versao anterior gravou -- senao a primeira checagem depois de
    /// um deploy perde a historia.
    #[test]
    fn estado_antigo_sem_campos_novos_ainda_le() {
        let dir = std::env::temp_dir().join(format!("logi-ota-velho-{}", agora()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            caminho(&dir),
            r#"{"quando_unix":1,"sistema":"linux","n_aqui":1,"build_aqui":"a","resultado":"igual","mensagem":"x"}"#,
        )
        .unwrap();
        let e = ler(&dir).expect("os campos com default tem de cobrir");
        assert_eq!(e.n_la, 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
