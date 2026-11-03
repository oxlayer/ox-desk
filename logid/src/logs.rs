//! Leitura do journal para a GUI.
//!
//! Existe porque a falta disso custou um diagnostico errado: a troca de
//! canal levava 105 s e o usuario, sem ver nada, concluiu que falhou. O
//! painel tem de mostrar o que o daemon esta fazendo.

use serde::Serialize;
use std::process::Command;

const UNIT: &str = "logid.service";
const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = 1000;

#[derive(Debug, Serialize)]
pub struct Entry {
    /// ISO curto, como o journal entrega.
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

/// Linhas que só poluem: o eco de sinal do D-Bus e a prova de vida.
fn noise(msg: &str) -> bool {
    msg.starts_with("border[sinal]") || msg.contains("border: vivo")
}

pub fn tail(lines: usize) -> Logs {
    let n = lines.clamp(1, MAX_LINES);
    let out = Command::new("journalctl")
        .args([
            "--user",
            "-u",
            UNIT,
            "-n",
            &n.to_string(),
            "--no-pager",
            "-o",
            "short-iso",
        ])
        .output();

    let Ok(out) = out else {
        return Logs {
            unit: UNIT.into(),
            entries: vec![],
            error: Some("nao consegui executar journalctl".into()),
        };
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines() {
        if line.starts_with("--") {
            continue; // cabecalho do journal
        }
        // "2026-11-03T18:51:18-0300 host logid[123]: mensagem"
        let (time, rest) = match line.split_once(' ') {
            Some((t, r)) => (t.to_string(), r),
            None => continue,
        };
        let msg = match rest.split_once("]: ") {
            Some((_, m)) => m,
            None => match rest.split_once(": ") {
                Some((_, m)) => m,
                None => rest,
            },
        };
        if noise(msg) {
            continue;
        }
        entries.push(Entry {
            time: time.get(11..19).unwrap_or(&time).to_string(),
            level: classify(msg),
            message: msg.to_string(),
        });
    }

    Logs { unit: UNIT.into(), entries, error: None }
}

pub fn default_lines() -> usize {
    DEFAULT_LINES
}
