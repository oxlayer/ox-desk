//! Config em TOML, legivel e editavel a mao.
//!
//! Dois niveis separados de proposito (spec 001): config e o que o usuario
//! escolheu, runtime e o que foi descoberto. Aqui so vive config.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Como interpretar uma porta fechada. Tres estados porque dois davam
/// falso alarme: o SSH que nao existe aqui de proposito, e o RDP do
/// Windows desligado por escolha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    /// Tem que estar no ar; vermelho se cair.
    #[default]
    Up,
    /// Tem que estar fechada (ex.: porta de servidor nesta maquina, que e cliente).
    Down,
    /// Informativo; nunca alarma.
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCfg {
    pub label: String,
    pub port: u16,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCfg {
    pub name: String,
    /// Nome mDNS, **nunca IP**. Ver spec 007.
    pub addr: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub services: Vec<ServiceCfg>,
    /// Units systemd --user a inspecionar (so faz sentido na maquina local).
    #[serde(default)]
    pub units: Vec<String>,
    /// Alias do ~/.ssh/config, se der para buscar detalhe por SSH.
    #[serde(default)]
    pub ssh: Option<String>,
}

fn default_port() -> u16 {
    8787
}
fn default_refresh() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_refresh")]
    pub refresh_seconds: u64,
    #[serde(default)]
    pub hosts: Vec<HostCfg>,
}

pub const DEFAULT_TOML: &str = include_str!("../default-config.toml");

impl Config {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                PathBuf::from(home).join(".config")
            });
        base.join("logi").join("config.toml")
    }

    /// Le do disco; na primeira execucao grava o default e usa ele.
    pub fn load() -> Result<Self, String> {
        let p = Self::path();
        if !p.exists() {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("criar {dir:?}: {e}"))?;
            }
            std::fs::write(&p, DEFAULT_TOML).map_err(|e| format!("gravar {p:?}: {e}"))?;
            eprintln!("config inicial gravada em {}", p.display());
        }
        let raw = std::fs::read_to_string(&p).map_err(|e| format!("ler {p:?}: {e}"))?;
        toml::from_str(&raw).map_err(|e| format!("TOML invalido em {}: {e}", p.display()))
    }

    /// Grava validando antes: se o TOML resultante nao reabre, nao escreve.
    pub fn save(&self) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| format!("serializar: {e}"))?;
        toml::from_str::<Config>(&text).map_err(|e| format!("auto-validacao falhou: {e}"))?;
        let p = Self::path();
        std::fs::write(&p, text).map_err(|e| format!("gravar {p:?}: {e}"))
    }
}
