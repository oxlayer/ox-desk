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
    /// Onde esta maquina fica na grade da GUI. Ver `Config::neighbors`.
    #[serde(default)]
    pub layout: Option<Cell>,
    /// windows | linux | macos. Opcional: ver `HostCfg::os`.
    #[serde(default)]
    pub os: Option<String>,
    /// Como controlar a tela deste host, quando der.
    #[serde(default)]
    pub screen: Option<ScreenCfg>,
    /// Esta entrada e a maquina onde o daemon roda?
    ///
    /// Era implicito -- `addr = "127.0.0.1"` e um texto livre em `role`
    /// dizendo "este PC". Passou a importar quando o daemon foi rodar no
    /// Windows: o dead-man's switch precisa saber para onde devolver o
    /// dispositivo, e "o host que nao sou eu" exige saber quem eu sou.
    ///
    /// Opcional: sem a chave, vale a heuristica do endereco de loopback,
    /// para as configs que ja existem continuarem valendo.
    #[serde(default)]
    pub local: Option<bool>,
}

impl HostCfg {
    /// SO deste host. A chave `os` manda; sem ela, inferimos.
    ///
    /// A inferencia existe para as configs que ja estao escritas: o host
    /// local e o SO em que este binario roda, e um `agent` com `%VAR%` ou
    /// `\\` so pode ser caminho de Windows. Quando nao da para saber,
    /// `Unknown` -- e quem usa degrada em vez de chutar e rodar o comando
    /// errado na maquina de outra pessoa.
    pub fn os(&self) -> HostOs {
        if let Some(v) = &self.os {
            return match v.to_ascii_lowercase().as_str() {
                "windows" | "win" => HostOs::Windows,
                "linux" => HostOs::Linux,
                "macos" | "mac" | "darwin" => HostOs::Macos,
                _ => HostOs::Unknown,
            };
        }
        if self.is_local() {
            return if cfg!(windows) {
                HostOs::Windows
            } else if cfg!(target_os = "macos") {
                HostOs::Macos
            } else {
                HostOs::Linux
            };
        }
        match &self.agent {
            Some(a) if a.contains('%') || a.contains('\\') => HostOs::Windows,
            _ => HostOs::Unknown,
        }
    }

    pub fn is_local(&self) -> bool {
        if let Some(v) = self.local {
            return v;
        }
        matches!(self.addr.as_str(), "127.0.0.1" | "::1" | "localhost")
    }
}

/// Posicao do host na grade.
///
/// Escrever `edge = "right"` e `target = "windows-host"` a mao sempre foi
/// descrever uma relacao ESPACIAL em prosa. A grade diz a mesma coisa de
/// um jeito que a pessoa ja pensa -- "o Windows fica a direita" -- e as
/// bordas saem dela.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Cell {
    pub col: i32,
    pub row: i32,
}

/// Sistema operacional do host.
///
/// Passou a importar quando o par deixou de ser sempre Windows. Dois
/// lugares montam comando para rodar NO par, e os dois sao especificos:
/// `agent::devices` monta PowerShell (`& "exe" devices`) e
/// `probe::ssh_detail` roda um one-liner de PowerShell com CIM. Contra
/// Ubuntu ou macOS os dois falhariam -- e falhariam em silencio, porque
/// o detalhe e enfeite e o probe do agente ja degrada para "nao
/// respondeu".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    Linux,
    Macos,
    Unknown,
}

/// Protocolo de tela do host, e em que porta.
///
/// Um por host porque e propriedade do SO: o Windows fala RDP, o macOS
/// fala VNC (Compartilhamento de Tela), o GNOME fala RDP pelo
/// `gnome-remote-desktop`. Nao ha um protocolo que os tres sirvam, e
/// escolher o nativo de cada um evita subir servidor novo em qualquer
/// deles -- a decisao esta em specs/006.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenCfg {
    /// rdp | vnc
    pub kind: String,
    #[serde(default = "default_screen_port")]
    pub port: u16,
    /// Usuario para o cliente ja preencher. Nao e credencial: senha
    /// nunca entra nesta config.
    #[serde(default)]
    pub user: Option<String>,
}

fn default_screen_port() -> u16 {
    3389
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
    /// Quantas ativacoes na janela antes de trocar. `1` = imediato.
    ///
    /// O mutter dispara no contato e nao tem dwell; aproximamos pela
    /// densidade de ativacoes. Mais alto evita troca por raspao, mais
    /// baixo responde na hora.
    #[serde(default = "default_hold_hits")]
    pub hold_hits: usize,
    #[serde(default = "default_hold_window_ms")]
    pub hold_window_ms: u64,
}

fn default_hold_hits() -> usize {
    2
}
fn default_hold_window_ms() -> u64 {
    300
}

impl Default for BorderCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            edge: default_edge(),
            target: None,
            hold_hits: default_hold_hits(),
            hold_window_ms: default_hold_window_ms(),
        }
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

/// Troca valores dentro de uma secao, preservando o resto do arquivo.
///
/// Terceira vez que esta cirurgia apareceu (grade, borda, agora o
/// reconciliador), entao virou funcao. O motivo e sempre o mesmo:
/// reserializar a struct produz TOML correto e apaga os comentarios, e
/// nesta config eles sao o registro de por que cada escolha foi feita.
///
/// Chave que nao existe na secao e **acrescentada** ao fim dela, nao
/// ignorada -- `leader` pode nunca ter sido escrito.
pub fn set_in_section(toml_text: &str, secao: &str, pares: &[(&str, String)]) -> String {
    let cabecalho = format!("[{secao}]");
    let mut out: Vec<String> = Vec::new();
    let mut dentro = false;
    let mut vistos: Vec<&str> = Vec::new();

    let faltantes = |vistos: &Vec<&str>| -> Vec<String> {
        pares
            .iter()
            .filter(|(k, _)| !vistos.contains(k))
            .map(|(k, v)| format!("{k} = {v}"))
            .collect()
    };

    for linha in toml_text.lines() {
        let t = linha.trim();
        if t.starts_with('[') {
            if dentro {
                out.extend(faltantes(&vistos));
            }
            dentro = t == cabecalho;
            vistos.clear();
            out.push(linha.to_string());
            continue;
        }
        if dentro {
            if let Some((chave, _)) = t.split_once('=') {
                let chave = chave.trim();
                if let Some((k, v)) = pares.iter().find(|(k, _)| *k == chave) {
                    out.push(format!("{k} = {v}"));
                    vistos.push(k);
                    continue;
                }
            }
        }
        out.push(linha.to_string());
    }
    if dentro {
        out.extend(faltantes(&vistos));
    }

    let mut texto = out.join("\n");
    if toml_text.ends_with('\n') && !texto.ends_with('\n') {
        texto.push('\n');
    }
    texto
}

fn gravar(depois: String) -> Result<(), String> {
    // Valida ANTES de gravar: edicao de texto que produz TOML invalido
    // deixaria o daemon sem config no proximo boot.
    toml::from_str::<Config>(&depois).map_err(|e| format!("a edicao quebrou o TOML: {e}"))?;
    let p = Config::path();
    std::fs::write(&p, depois).map_err(|e| format!("gravar {p:?}: {e}"))
}

pub fn save_border_enabled(on: bool) -> Result<(), String> {
    let p = Config::path();
    let antes = std::fs::read_to_string(&p).map_err(|e| format!("ler {p:?}: {e}"))?;
    gravar(set_in_section(&antes, "input.border", &[("enabled", on.to_string())]))
}

/// Liga/desliga o reconciliador e escolhe quem manda.
///
/// Configuravel porque a resposta certa depende de como a pessoa troca
/// de maquina: com a borda ligada quem manda e o mouse (e ele que
/// atravessa); trocando no botao Easy-Switch do teclado, o lider tem de
/// ser o TECLADO -- senao o reconciliador puxa o teclado de volta para
/// onde o mouse esta e desfaz a acao que a pessoa acabou de fazer.
pub fn save_follow(on: bool, leader: Option<&str>) -> Result<(), String> {
    let p = Config::path();
    let antes = std::fs::read_to_string(&p).map_err(|e| format!("ler {p:?}: {e}"))?;
    let mut pares: Vec<(&str, String)> = vec![("enabled", on.to_string())];
    if let Some(l) = leader {
        pares.push(("leader", format!("\"{}\"", l.replace('"', ""))));
    }
    gravar(set_in_section(&antes, "input.follow", &pares))
}

/// Grava as posicoes no arquivo, preservando comentarios.
pub fn save_layout(pos: &BTreeMap<String, Cell>) -> Result<(), String> {
    let p = Config::path();
    let antes = std::fs::read_to_string(&p).map_err(|e| format!("ler {p:?}: {e}"))?;
    let depois = apply_layout(&antes, pos);
    // Valida ANTES de gravar: uma edicao de texto que produz TOML
    // invalido deixaria o daemon sem config no proximo boot.
    toml::from_str::<Config>(&depois).map_err(|e| format!("a edicao quebrou o TOML: {e}"))?;
    std::fs::write(&p, depois).map_err(|e| format!("gravar {p:?}: {e}"))
}

/// Escreve a posicao de cada host na grade, preservando o arquivo.
///
/// Edicao **cirurgica de texto**, nao serializacao. Reserializar a
/// struct produziria um TOML correto e jogaria fora todos os
/// comentarios -- e nesta config os comentarios sao o registro de por
/// que cada escolha foi feita (por que o canal 3 nao esta mapeado, por
/// que o follow esta desligado no Windows, por que nunca se escreve IP).
/// Perder isso para salvar duas coordenadas seria pessimo negocio.
///
/// Para cada `[[hosts]]` com `name` conhecido: troca a linha `layout` se
/// houver, senao insere logo depois do `name`.
pub fn apply_layout(toml_text: &str, pos: &BTreeMap<String, Cell>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut host_atual: Option<String> = None;
    let mut ja_escrito = false;

    for linha in toml_text.lines() {
        let t = linha.trim();

        // Um cabecalho de tabela qualquer fecha o host anterior.
        if t.starts_with("[[") || (t.starts_with('[') && !t.starts_with("[[")) {
            if t == "[[hosts]]" {
                host_atual = None;
                ja_escrito = false;
            } else if !t.starts_with("[hosts.") && !t.starts_with("[[hosts.") {
                host_atual = None;
            }
            out.push(linha.to_string());
            continue;
        }

        // Dentro de um [[hosts]], a linha `name` identifica quem e.
        if host_atual.is_none() {
            if let Some(nome) = t
                .strip_prefix("name")
                .and_then(|r| r.trim_start().strip_prefix('='))
                .map(|r| r.trim().trim_matches('"').to_string())
            {
                if pos.contains_key(&nome) {
                    host_atual = Some(nome.clone());
                    out.push(linha.to_string());
                    let c = pos[&nome];
                    out.push(format!("layout = {{ col = {}, row = {} }}", c.col, c.row));
                    ja_escrito = true;
                    continue;
                }
            }
            out.push(linha.to_string());
            continue;
        }

        // Ja escrevemos a nova: a antiga, se existir, e descartada.
        if ja_escrito && t.starts_with("layout") && t.contains('=') {
            continue;
        }
        out.push(linha.to_string());
    }

    let mut texto = out.join("\n");
    if toml_text.ends_with('\n') && !texto.ends_with('\n') {
        texto.push('\n');
    }
    texto
}

impl Config {
    /// Vizinhos da maquina local na grade: (borda, nome do host).
    ///
    /// "Imediatamente a direita" e o host mais PROXIMO com `col` maior na
    /// mesma linha -- nao `col + 1`. Com buraco na grade (tres maquinas
    /// em 0, 1 e 3, por exemplo) exigir adjacencia exata deixaria a borda
    /// sem alvo, e a pessoa arrastou os quadrados justamente para dizer
    /// quem fica de que lado.
    ///
    /// Hosts sem `layout` ficam de fora: nao da para derivar direcao de
    /// quem nao tem posicao.
    pub fn neighbors(&self) -> Vec<(&'static str, String)> {
        let Some(eu) = self.hosts.iter().find(|h| h.is_local()) else { return Vec::new() };
        let Some(meu) = eu.layout else { return Vec::new() };

        let mut v = Vec::new();
        for (borda, mesma_linha, maior) in [
            ("right", true, true),
            ("left", true, false),
            ("bottom", false, true),
            ("top", false, false),
        ] {
            let mut cands: Vec<(i32, &HostCfg)> = self
                .hosts
                .iter()
                .filter(|h| !h.is_local())
                .filter_map(|h| h.layout.map(|c| (c, h)))
                .filter(|(c, _)| if mesma_linha { c.row == meu.row } else { c.col == meu.col })
                .filter_map(|(c, h)| {
                    let d = if mesma_linha { c.col - meu.col } else { c.row - meu.row };
                    let ok = if maior { d > 0 } else { d < 0 };
                    ok.then(|| (d.abs(), h))
                })
                .collect();
            cands.sort_by_key(|(d, _)| *d);
            if let Some((_, h)) = cands.first() {
                v.push((borda, h.name.clone()));
            }
        }
        v
    }

    /// As bordas a vigiar: `(borda, host alvo)`.
    ///
    /// A grade manda quando existe. Sem ela, vale o par `edge`/`target`
    /// escrito a mao -- as configs anteriores a grade continuam valendo,
    /// e quem quiser uma borda so continua podendo escreve-la.
    pub fn border_edges(&self) -> Vec<(String, String)> {
        self.border_edges_full().into_iter().filter(|(_, _, ok)| *ok).map(|(e, t, _)| (e, t)).collect()
    }

    /// Como `border_edges`, mas sem esconder os vizinhos descartados.
    ///
    /// O terceiro campo e "este alvo consegue RECEBER". Um host sem
    /// `ssh` + `agent` nao tem quem devolva o dispositivo, entao o
    /// `focus::plan` bloqueia a troca -- criar barreira para ele daria
    /// uma borda que so sabe recusar, enchendo o log a cada vez que a
    /// pessoa encosta. E caso real: o MacBook nao tem receptor Logitech.
    ///
    /// A UI mostra os dois grupos, para a grade nao mentir em silencio
    /// sobre um vizinho que esta la e nao funciona.
    pub fn border_edges_full(&self) -> Vec<(String, String, bool)> {
        let b = &self.input.border;
        if !b.enabled {
            return Vec::new();
        }
        let recebe = |nome: &str| {
            self.hosts
                .iter()
                .find(|h| h.name == nome)
                .is_some_and(|h| h.ssh.is_some() && h.agent.is_some())
        };
        let da_grade = self.neighbors();
        if !da_grade.is_empty() {
            return da_grade
                .into_iter()
                .map(|(e, h)| {
                    let ok = recebe(&h);
                    (e.to_string(), h, ok)
                })
                .collect();
        }
        match &b.target {
            Some(t) => {
                let ok = recebe(t);
                vec![(b.edge.clone(), t.clone(), ok)]
            }
            None => Vec::new(),
        }
    }

    /// O primeiro host que nao e esta maquina.
    ///
    /// Quem usa e o dead-man's switch da borda do Windows, que precisa
    /// saber para onde devolver o dispositivo. No Linux ninguem chama:
    /// la o guard mora no alvo (o `logid-agent`), nao na origem.
    ///
    /// Com dois hosts e o par, sem ambiguidade. Com mais de dois, a
    /// escolha passa a ter de ser explicita e quem chama tem de dizer
    /// qual -- por isso devolve o primeiro em vez de adivinhar peso.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn peer_host(&self) -> Option<&HostCfg> {
        self.hosts.iter().find(|h| !h.is_local())
    }

    pub fn path() -> PathBuf {
        crate::sys::config_dir().join("config.toml")
    }

    /// Le do disco; na primeira execucao grava o default e usa ele.
    pub fn load() -> Result<Self, String> {
        let p = Self::path();
        if !p.exists() {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("criar {dir:?}: {e}"))?;
            }
            std::fs::write(&p, DEFAULT_TOML).map_err(|e| format!("gravar {p:?}: {e}"))?;
            gripe!("config inicial gravada em {}", p.display());
        }
        let raw = std::fs::read_to_string(&p).map_err(|e| format!("ler {p:?}: {e}"))?;
        toml::from_str(&raw).map_err(|e| format!("TOML invalido em {}: {e}", p.display()))
    }

    /// Grava validando antes: se o TOML resultante nao reabre, nao escreve.
    /// Grava a config inteira, reserializada.
    ///
    /// ⚠️ **Destroi todos os comentarios do arquivo.** Reserializar a
    /// struct produz TOML correto e perde tudo que nao e dado -- e nesta
    /// config os comentarios registram por que cada escolha foi feita.
    ///
    /// Hoje so o `PATCH /api/config` chama isto, e nada na UI usa aquela
    /// rota. Para mudancas pontuais prefira edicao cirurgica de texto,
    /// como `apply_layout` faz.
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
    fn quem_sou_eu_por_loopback_ou_por_flag() {
        // Heuristica: configs que ja existem nao tem a chave `local`.
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "aqui"
            addr = "127.0.0.1"
            [[hosts]]
            name = "la"
            addr = "la.local"
            "#,
        )
        .unwrap();
        assert!(c.hosts[0].is_local());
        assert!(!c.hosts[1].is_local());
        assert_eq!(c.peer_host().map(|h| h.name.as_str()), Some("la"));

        // Flag explicita vence o endereco -- e o caso do Windows, onde a
        // entrada local pode nao usar loopback. Uma segunda leitura
        // divergente deste criterio ja existiu em `follow.rs` e foi
        // removida; este teste e o que impede ela de voltar.
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "aqui"
            addr = "windows-host.local"
            local = true
            [[hosts]]
            name = "la"
            addr = "127.0.0.1"
            local = false
            "#,
        )
        .unwrap();
        assert!(c.hosts[0].is_local(), "flag explicita tem de vencer o endereco");
        assert!(!c.hosts[1].is_local(), "local = false vence o loopback");
        assert_eq!(c.peer_host().map(|h| h.name.as_str()), Some("la"));
    }

    #[test]
    fn salvar_a_grade_nao_apaga_comentario() {
        let antes = r#"# Config do logid. Comentario que explica uma decisao cara.
port = 8787

[[hosts]]
name = "aqui"
addr = "127.0.0.1"
# Este comentario tem de sobreviver.
local = true

  [[hosts.services]]
  label = "SSH"
  port = 22

[[hosts]]
name = "la"
layout = { col = 9, row = 9 }
addr = "la.local"
"#;
        let mut pos = BTreeMap::new();
        pos.insert("aqui".to_string(), Cell { col: 0, row: 1 });
        pos.insert("la".to_string(), Cell { col: 2, row: 1 });
        let depois = apply_layout(antes, &pos);

        assert!(depois.contains("# Comentario que explica uma decisao cara.")
                || depois.contains("Comentario que explica uma decisao cara"));
        assert!(depois.contains("# Este comentario tem de sobreviver."));
        assert!(depois.contains("label = \"SSH\""), "subtabela intacta");
        // Inserida onde nao havia.
        assert!(depois.contains("name = \"aqui\"\nlayout = { col = 0, row = 1 }"));
        // Substituida onde havia -- e a antiga sumiu.
        assert!(depois.contains("layout = { col = 2, row = 1 }"));
        assert!(!depois.contains("col = 9"), "a posicao antiga tem de sair");
        // E continua sendo TOML valido com o resto no lugar.
        let c: Config = toml::from_str(&depois).unwrap();
        assert_eq!(c.hosts.len(), 2);
        assert_eq!(c.hosts[0].services.len(), 1);
        assert_eq!(c.hosts[1].layout.map(|l| (l.col, l.row)), Some((2, 1)));
    }

    #[test]
    fn chave_que_falta_e_acrescentada() {
        // `leader` pode nunca ter sido escrito na secao.
        let antes = "[input.follow]\n# comentario\nenabled = false\n";
        let depois = set_in_section(
            antes,
            "input.follow",
            &[("enabled", "true".into()), ("leader", "\"MX Keys\"".into())],
        );
        assert!(depois.contains("# comentario"));
        assert!(depois.contains("enabled = true"));
        assert!(depois.contains("leader = \"MX Keys\""));
    }

    #[test]
    fn toggle_da_borda_preserva_a_secao() {
        let antes = "# topo\n[input.border]\n# por que esta assim\nenabled = false\nedge = \"right\"\n\n[input.follow]\nenabled = true\n";
        let depois = set_in_section(antes, "input.border", &[("enabled", "true".into())]);
        assert!(depois.contains("# por que esta assim"));
        assert!(depois.contains("[input.border]\n# por que esta assim\nenabled = true"));
        // Nao pode encostar no enabled do OUTRO bloco.
        assert!(depois.contains("[input.follow]\nenabled = true"));
        let c: Config = toml::from_str(&depois).unwrap();
        assert!(c.input.border.enabled);
        assert!(c.input.follow.enabled);

        // E desligar de volta tambem.
        let c2: Config = toml::from_str(&set_in_section(&depois, "input.border", &[("enabled", "false".into())])).unwrap();
        assert!(!c2.input.border.enabled);
        assert!(c2.input.follow.enabled, "o follow nao pode ser arrastado junto");
    }

    #[test]
    fn host_fora_do_mapa_fica_como_estava() {
        let antes = "[[hosts]]\nname = \"intocado\"\naddr = \"x.local\"\n";
        let depois = apply_layout(antes, &BTreeMap::new());
        assert_eq!(antes, depois);
    }

    #[test]
    fn vizinhos_saem_da_grade() {
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "eu"
            addr = "127.0.0.1"
            layout = { col = 1, row = 1 }
            [[hosts]]
            name = "direita"
            addr = "d.local"
            layout = { col = 3, row = 1 }
            [[hosts]]
            name = "esquerda"
            addr = "e.local"
            layout = { col = 0, row = 1 }
            [[hosts]]
            name = "acima"
            addr = "a.local"
            layout = { col = 1, row = 0 }
            [[hosts]]
            name = "diagonal"
            addr = "x.local"
            layout = { col = 2, row = 2 }
            [[hosts]]
            name = "sem posicao"
            addr = "s.local"
            "#,
        )
        .unwrap();
        let n = c.neighbors();
        let acha = |b: &str| n.iter().find(|(e, _)| *e == b).map(|(_, h)| h.as_str());
        // Coluna 3 com buraco no 2: ainda e o vizinho da direita.
        assert_eq!(acha("right"), Some("direita"));
        assert_eq!(acha("left"), Some("esquerda"));
        assert_eq!(acha("top"), Some("acima"));
        // Diagonal nao e vizinho de borda nenhuma.
        assert_eq!(acha("bottom"), None);
        assert_eq!(n.len(), 3, "so as tres bordas com vizinho");
    }

    #[test]
    fn o_mais_proximo_ganha() {
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "eu"
            addr = "127.0.0.1"
            layout = { col = 0, row = 0 }
            [[hosts]]
            name = "longe"
            addr = "l.local"
            layout = { col = 5, row = 0 }
            [[hosts]]
            name = "perto"
            addr = "p.local"
            layout = { col = 1, row = 0 }
            "#,
        )
        .unwrap();
        assert_eq!(
            c.neighbors().iter().find(|(e, _)| *e == "right").map(|(_, h)| h.as_str()),
            Some("perto")
        );
    }

    #[test]
    fn sem_grade_nenhuma_nao_ha_vizinho() {
        // Config antiga, sem `layout`: a borda explicita continua valendo.
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "eu"
            addr = "127.0.0.1"
            [[hosts]]
            name = "outro"
            addr = "o.local"
            "#,
        )
        .unwrap();
        assert!(c.neighbors().is_empty());
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
