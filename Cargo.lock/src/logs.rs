//! Log para a GUI: a politica mora aqui, a origem na plataforma.
//!
//! Existe porque a falta disso custou um diagnostico errado: a troca de
//! canal levava 105 s e o usuario, sem ver nada, concluiu que falhou. O
//! painel tem de mostrar o que o daemon esta fazendo.
//!
//! A divisao e deliberada. `sys::log_lines` entrega pares (hora,
//! mensagem) -- journal no Linux, arquivo no Windows -- e a classificacao
//! e o filtro de ruido ficam aqui, uma vez. Duas copias da heuristica
//! divergiriam, e a primeira vez que divergissem eu nao notaria.

use serde::Serialize;

const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = 1000;

/// O que a plataforma entrega: ja em ordem, ja sem cabecalho.
pub struct Raw {
    /// De onde veio -- unit do systemd ou caminho do arquivo.
    pub unit: String,
    pub lines: Vec<(String, String)>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub time: String,
    pub message: String,
    /// info | warn | error -- heuristica sobre o texto, porque o daemon
    /// escreve em stdout/stderr e nao usa prioridade do syslog.
    pub level: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Logs {
    pub unit: String,
    pub entries: Vec<Entry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn classify(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("falhou") || m.contains("error") || m.contains("fatal") || m.contains("nao trocou")
    {
        "error"
    } else if m.contains("aviso")
        || m.contains("estado dividido")
        || m.contains("cooldown")
        || m.contains("desarmada")
    {
        "warn"
    } else {
        "info"
    }
}

/// Linhas que so poluem: o eco de sinal do D-Bus e a prova de vida.
fn noise(msg: &str) -> bool {
    msg.starts_with("border[sinal]") || msg.contains("border: vivo")
}

pub fn tail(lines: usize) -> Logs {
    let raw = crate::sys::log_lines(lines.clamp(1, MAX_LINES));
    let entries = raw
        .lines
        .into_iter()
        .filter(|(_, msg)| !noise(msg))
        .map(|(time, message)| Entry { time, level: classify(&message), message })
        .collect();
    Logs { unit: raw.unit, entries, error: raw.error }
}

pub fn default_lines() -> usize {
    DEFAULT_LINES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifica_pelo_texto() {
        assert_eq!(classify("slot 1 FALHOU: sem resposta"), "error");
        assert_eq!(classify("border: cooldown ate 18:00"), "warn");
        assert_eq!(classify("troca concluida em 46ms"), "info");
    }

    #[test]
    fn ruido_sai() {
        assert!(noise("border[sinal] Activated x=2560.0"));
        assert!(noise("border: vivo"));
        assert!(!noise("border: permanencia 1/1, trocando"));
    }
}
