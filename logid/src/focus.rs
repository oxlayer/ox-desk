//! Passar o foco de input para outra maquina.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! Dry-run por padrao. A rota fisica e IRREVERSIVEL do lado de origem
//! (depois do ChangeHost este receptor nao alcanca mais o dispositivo),
//! entao o pre-flight substitui o rollback -- nao ha o que desfazer.

use crate::config::Config;
use crate::hidpp;
use crate::probe;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// Troca de canal Easy-Switch via HID++ 0x1814.
    Physical,
    /// Eventos de input pela rede (Deskflow).
    Lan,
    /// Nenhuma rota viavel.
    None,
}

#[derive(Debug, Deserialize)]
pub struct FocusRequest {
    pub host: String,
    #[serde(default)]
    pub route: Option<Route>,
    /// Sem isto, so devolve o plano. Nunca executa por acidente.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    /// Falha bloqueante impede a execucao.
    pub blocking: bool,
}

impl Check {
    fn pass(name: &str, detail: impl Into<String>) -> Self {
        Check { name: name.into(), ok: true, detail: detail.into(), blocking: false }
    }
    fn block(name: &str, detail: impl Into<String>) -> Self {
        Check { name: name.into(), ok: false, detail: detail.into(), blocking: true }
    }
    fn warn(name: &str, detail: impl Into<String>) -> Self {
        Check { name: name.into(), ok: false, detail: detail.into(), blocking: false }
    }
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub target: String,
    pub route: Route,
    pub checks: Vec<Check>,
    /// Todos os bloqueantes passaram.
    pub ready: bool,
    /// Se executado, nao ha como desfazer deste lado.
    pub irreversible: bool,
    /// O que seria feito, em ordem.
    pub actions: Vec<String>,
    /// Como recuperar se der errado -- mostrar ANTES, nao depois.
    pub recovery: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub plan: Plan,
    pub executed: bool,
    pub results: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_local(h: &crate::config::HostCfg) -> bool {
    h.addr == "127.0.0.1" || h.addr == "localhost" || h.addr == "::1"
}

pub fn plan(cfg: &Config, req: &FocusRequest) -> Plan {
    let mut checks = Vec::new();
    let mut actions = Vec::new();
    let recovery = vec![
        "aperte o botao Easy-Switch no proprio dispositivo (sempre funciona)".into(),
        "ou plugue um teclado USB".into(),
        "ou entre por SSH de outra maquina".into(),
    ];

    let Some(target) = cfg.hosts.iter().find(|h| h.name == req.host) else {
        checks.push(Check::block("host", format!("'{}' nao esta na config", req.host)));
        return Plan {
            target: req.host.clone(),
            route: Route::None,
            ready: false,
            irreversible: false,
            checks,
            actions,
            recovery,
        };
    };

    if is_local(target) {
        checks.push(Check::block("host", "o alvo e esta propria maquina"));
        return Plan {
            target: req.host.clone(),
            route: Route::None,
            ready: false,
            irreversible: false,
            checks,
            actions,
            recovery,
        };
    }
    checks.push(Check::pass("host", format!("{} ({})", target.name, target.addr)));

    // Alcance: qualquer servico esperado no ar basta para dizer que esta viva.
    let reachable = target
        .services
        .iter()
        .any(|s| probe::tcp_probe(&target.addr, s.port).0);
    if reachable {
        checks.push(Check::pass("alcance", format!("{} responde", target.addr)));
    } else {
        checks.push(Check::block("alcance", format!("{} nao responde", target.addr)));
    }

    let local_name = cfg
        .hosts
        .iter()
        .find(|h| is_local(h))
        .map(|h| h.name.clone())
        .unwrap_or_default();

    let want = req.route.unwrap_or(Route::Physical);
    let route = match want {
        Route::Lan => Route::Lan,
        _ => Route::Physical,
    };

    match route {
        Route::Physical => {
            let survey = hidpp::survey();
            if survey.hidraw_access {
                checks.push(Check::pass("hidraw", "ACL de uaccess presente"));
            } else {
                checks.push(Check::block(
                    "hidraw",
                    survey.error.clone().unwrap_or_else(|| "sem acesso".into()),
                ));
            }

            if cfg.devices.is_empty() {
                checks.push(Check::block("mapa de canais", "nenhum [[devices]] na config"));
            }

            for dcfg in &cfg.devices {
                let Some(&chan) = dcfg.hosts.get(&req.host) else {
                    checks.push(Check::block(
                        "mapa de canais",
                        format!("'{}' nao tem canal declarado para '{}'", dcfg.name, req.host),
                    ));
                    continue;
                };
                let found = survey
                    .receivers
                    .iter()
                    .flat_map(|r| r.devices.iter())
                    .find(|d| d.name == dcfg.name);
                match found {
                    None => checks.push(Check::block(
                        "posse do dispositivo",
                        format!("'{}' nao esta pareado neste receptor", dcfg.name),
                    )),
                    Some(d) if !d.online => checks.push(Check::block(
                        "posse do dispositivo",
                        format!(
                            "'{}' esta offline aqui -- ja esta em outro canal, \
                             e HID++ nao alcanca dispositivo offline",
                            dcfg.name
                        ),
                    )),
                    Some(d) => {
                        checks.push(Check::pass(
                            "posse do dispositivo",
                            format!("'{}' online no slot {}", d.name, d.slot),
                        ));
                        actions.push(format!(
                            "solaar config {} change-host {}  ({} -> canal {} em {})",
                            d.slot, chan, d.name, chan, req.host
                        ));
                    }
                }
                if !local_name.is_empty() && !dcfg.hosts.contains_key(&local_name) {
                    checks.push(Check::warn(
                        "volta",
                        format!(
                            "'{}' nao tem canal declarado para '{}': sem isso \
                             ninguem sabe como traze-lo de volta",
                            dcfg.name, local_name
                        ),
                    ));
                }
            }

            // O bloqueio estrutural da fase 3: ver spec 005.
            checks.push(Check::block(
                "dead-man's switch no alvo",
                "o agente do Windows ainda nao existe. Sem ele, ninguem \
                 devolve o dispositivo se o alvo nao receber input -- e a \
                 origem NAO pode desfazer. Rota fisica e de ida so.",
            ));
        }
        Route::Lan => {
            let up = probe::tcp_probe(&target.addr, 24800).0;
            if up {
                checks.push(Check::pass("deskflow", "servidor na 24800 responde"));
            } else {
                checks.push(Check::block("deskflow", "nada na 24800 do alvo"));
            }
            // Honestidade sobre o que o Deskflow permite:
            checks.push(Check::block(
                "troca programatica",
                "o Deskflow troca de tela por movimento do cursor ou por \
                 hotkey configurada NO SERVIDOR -- nao por comando externo. \
                 Nao ha como forcar o foco pela rota LAN daqui.",
            ));
        }
        Route::None => {}
    }

    let ready = !checks.iter().any(|c| c.blocking && !c.ok);
    Plan {
        target: req.host.clone(),
        route,
        irreversible: route == Route::Physical,
        ready,
        checks,
        actions,
        recovery,
    }
}

pub fn execute(cfg: &Config, req: &FocusRequest) -> Outcome {
    let plan = plan(cfg, req);

    if !req.confirm {
        return Outcome { plan, executed: false, results: vec![], error: None };
    }
    if !plan.ready {
        let n = plan.checks.iter().filter(|c| c.blocking && !c.ok).count();
        return Outcome {
            plan,
            executed: false,
            results: vec![],
            error: Some(format!("{n} verificacao(oes) bloqueante(s) falhou(aram)")),
        };
    }
    if plan.route != Route::Physical {
        return Outcome {
            plan,
            executed: false,
            results: vec![],
            error: Some("so a rota fisica e executavel".into()),
        };
    }

    // Ponto sem volta.
    let mut results = Vec::new();
    let survey = hidpp::survey();
    for dcfg in &cfg.devices {
        let Some(&chan) = dcfg.hosts.get(&req.host) else { continue };
        let Some(d) = survey
            .receivers
            .iter()
            .flat_map(|r| r.devices.iter())
            .find(|d| d.name == dcfg.name && d.online)
        else {
            continue;
        };
        match hidpp::change_host(d.slot, chan) {
            Ok(_) => results.push(format!("{}: slot {} -> canal {}", d.name, d.slot, chan)),
            Err(e) => results.push(format!("{}: FALHOU -- {e}", d.name)),
        }
    }
    Outcome { plan, executed: true, results, error: None }
}
