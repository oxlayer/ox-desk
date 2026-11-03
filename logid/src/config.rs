//! Config em TOML, legivel e editavel a mao.
//!
//! Dois niveis separados de proposito (spec 001): config e o que o usuario
//! escolheu, runtime e o que foi descoberto. Aqui so vive config.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    /// Caminho do logid-agent nesta maquina. Sem ele, nao ha dead-man's
    /// switch -- e sem dead-man's switch a rota fisica e de ida so.
    #[serde(default)]
    pub agent: Option<String>,
}

fn default_edge() -> String {
    "right".into()
}

/// Vigilancia de borda. **Desligada por padrao** de proposito: ela dispara
/// troca de canal, que e irreversivel do lado de origem (spec 005).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BorderCfg {
    #[serde(default)]
    pub enabled: bool,
    /// left | right | top | bottom
    #[serde(default = "default_edge")]
    pub edge: String,
    /// Host que recebe o foco ao atravessar. Sem ele, nao arma.
    #[serde(default)]
    pub target: Option<String>,
}

impl Default for BorderCfg {
    fn default() -> Self {
        Self { enabled: false, edge: default_edge(), target: None }
    }
}

fn default_follow_interval() -> u64 {
    5
}

/// Fazer os demais dispositivos seguirem o canal do lider.
///
/// Resolve o estado dividido na raiz: mouse e teclado em maquinas
/// diferentes. E funciona inclusive quando a troca foi **manual**, no
/// botao Easy-Switch -- que o `border` nao ve.
///
/// **Desligado por padrao**: mover dispositivo e irreversivel do lado de
/// origem (spec 005), e um reconciliador que erra te deixa sem teclado.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowCfg {
    #[serde(default)]
    pub enabled: bool,
    /// Nome (HID++) do dispositivo que manda. Os outros de `[[devices]]`
    /// seguem. Sem ele, nao arma.
    #[serde(default)]
    pub leader: Option<String>,
    #[serde(default = "default_follow_interval")]
    pub interval_seconds: u64,
}

impl Default for FollowCfg {
    fn default() -> Self {
        Self { enabled: false, leader: None, interval_seconds: default_follow_interval() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputCfg {
    #[serde(default)]
    pub border: BorderCfg,
    #[serde(default)]
    pub follow: FollowCfg,
}

fn default_port() -> u16 {
    8787
}
fn default_refresh() -> u64 {
    10
}

/// Dispositivo Logitech e em que canal Easy-Switch ele vive em cada host.
///
/// Este mapa e exatamente o que o Solaar nao tem e o que o Logitech Flow
/// guarda na nuvem. Aqui mora num TOML seu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCfg {
    /// Nome como o Solaar reporta (ex.: "MX Anywhere 3").
    pub name: String,
    /// host -> canal (1-based).
    #[serde(default)]
    pub hosts: BTreeMap<String, u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_refresh")]
    pub refresh_seconds: u64,
    #[serde(default)]
    pub hosts: Vec<HostCfg>,
    #[serde(default)]
    pub devices: Vec<DeviceCfg>,
    #[serde(default)]
    pub input: InputCfg,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expect_default_e_up() {
        // Importa: um servico sem `expect` declarado tem de alarmar se cair.
        assert_eq!(Expect::default(), Expect::Up);
    }

    #[test]
    fn toml_minimo_parseia_e_aplica_defaults() {
        let t = r#"
            [[hosts]]
            name = "a"
            addr = "a.local"
              [[hosts.services]]
              label = "SSH"
              port = 22
        "#;
        let c: Config = toml::from_str(t).expect("deveria parsear");
        assert_eq!(c.port, 8787, "porta default");
        assert_eq!(c.refresh_seconds, 10);
        assert_eq!(c.hosts.len(), 1);
        assert_eq!(c.hosts[0].services[0].expect, Expect::Up);
        assert!(c.devices.is_empty());
        assert!(!c.input.border.enabled, "borda desligada por default");
    }

    #[test]
    fn expect_down_e_any_desserializam() {
        let t = r#"
            [[hosts]]
            name = "a"
            addr = "a.local"
              [[hosts.services]]
              label = "srv"
              port = 24800
              expect = "down"
              [[hosts.services]]
              label = "rdp"
              port = 3389
              expect = "any"
        "#;
        let c: Config = toml::from_str(t).unwrap();
        assert_eq!(c.hosts[0].services[0].expect, Expect::Down);
        assert_eq!(c.hosts[0].services[1].expect, Expect::Any);
    }

    #[test]
    fn mapa_de_canais_por_nome_de_dispositivo() {
        let t = r#"
            [[hosts]]
            name = "a"
            addr = "a.local"
            [[devices]]
            name = "MX Anywhere 3"
              [devices.hosts]
              "a" = 1
              "b" = 2
        "#;
        let c: Config = toml::from_str(t).unwrap();
        assert_eq!(c.devices[0].hosts.get("a"), Some(&1));
        assert_eq!(c.devices[0].hosts.get("b"), Some(&2));
    }
}
