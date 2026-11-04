//! Gerenciador de tarefas: o que esta rodando em cada maquina, e matar.
//!
//! Spec: ../specs/009-processes.md
//!
//! ## Uma linha de comando por SO, escolhida pelo SO do ALVO
//!
//! Nao existe um comando que os tres SOs entendam. A escolha sai de
//! `HostCfg::os`, nao do SO onde o daemon roda -- rodar `ps` contra o
//! Windows, ou `Get-Process` contra o Ubuntu, falha calado e o painel
//! fica vazio sem dizer por que.
//!
//! ## O que `cpu` significa muda por SO, e isso e declarado
//!
//! No Unix o `ps` da **percentual** instantaneo. No Windows o
//! `Get-Process` da **segundos acumulados** de CPU desde que o processo
//! subiu -- numero com outra unidade e outra leitura. Converter um no
//! outro exigiria duas amostras e nao vale o custo aqui; entao o campo
//! vem com `cpu_kind` dizendo qual dos dois e, e a UI rotula certo.
//! Mostrar "87" numa coluna "%" quando sao segundos seria mentira
//! plausivel, que e a pior especie.

use crate::config::HostOs;
use crate::remote;
use serde::Serialize;

/// Teto de processos devolvidos. A lista e ordenada por CPU, entao o que
/// importa esta no topo -- e 2000 linhas de JSON nao ajudam ninguem.
const LIMITE: usize = 60;
const TIMEOUT_S: u32 = 8;

#[derive(Debug, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cpu: f64,
    pub mem_mb: f64,
}

#[derive(Debug, Serialize)]
pub struct Procs {
    pub host: String,
    /// local | ssh
    pub source: &'static str,
    /// pct | seconds -- ver o cabecalho do modulo.
    pub cpu_kind: &'static str,
    pub procs: Vec<Proc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Procs {
    fn failed(host: &str, source: &'static str, msg: impl Into<String>) -> Self {
        Procs {
            host: host.into(),
            source,
            cpu_kind: "pct",
            procs: vec![],
            error: Some(msg.into()),
        }
    }
}

fn lista_cmd(os: HostOs) -> Option<(&'static str, &'static str)> {
    match os {
        // `comm=` sem cabecalho; ordenacao no `sort` porque a flag de
        // ordenar do ps difere entre Linux (`--sort`) e macOS (`-r`).
        HostOs::Linux | HostOs::Macos => Some((
            "ps -eo pid=,pcpu=,rss=,comm= | sort -k2 -rn | head -n 60",
            "pct",
        )),
        HostOs::Windows => Some((
            "Get-Process | Sort-Object CPU -Descending | Select-Object -First 60 \
             Id,ProcessName,CPU,WorkingSet64 | ConvertTo-Json -Compress",
            "seconds",
        )),
        HostOs::Unknown => None,
    }
}

pub fn list(cfg: &crate::config::Config, host: &str) -> Procs {
    let Some(h) = cfg.hosts.iter().find(|x| x.name == host) else {
        return Procs::failed(host, "local", format!("host '{host}' nao esta na config"));
    };
    let source = if h.is_local() { "local" } else { "ssh" };
    let Some((cmd, cpu_kind)) = lista_cmd(h.os()) else {
        return Procs::failed(
            host,
            source,
            "SO do host desconhecido: declare `os` em [[hosts]] para eu saber que comando usar",
        );
    };
    let saida = match remote::run(h, cmd, TIMEOUT_S) {
        Ok(s) => s,
        Err(e) => return Procs::failed(host, source, e),
    };
    let mut procs = if remote::is_powershell(h) { parse_ps_json(&saida) } else { parse_ps(&saida) };
    procs.truncate(LIMITE);
    Procs { host: host.into(), source, cpu_kind, procs, error: None }
}

/// `  1234  12.5  65432 nome do processo`
fn parse_ps(out: &str) -> Vec<Proc> {
    let mut v = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let (Ok(pid), Ok(cpu), Ok(rss)) =
            (f[0].parse::<u32>(), f[1].parse::<f64>(), f[2].parse::<f64>())
        else {
            continue;
        };
        // O nome pode ter espaco (caminho de app no macOS), entao e tudo
        // depois do terceiro campo. Fica so o ultimo componente: o
        // caminho inteiro de um app `.app` nao cabe na tela e nao informa.
        let bruto = f[3..].join(" ");
        let name = bruto.rsplit('/').next().unwrap_or(&bruto).to_string();
        v.push(Proc { pid, name, cpu, mem_mb: (rss / 1024.0 * 10.0).round() / 10.0 });
    }
    v
}

fn parse_ps_json(out: &str) -> Vec<Proc> {
    // Um unico processo faz o ConvertTo-Json devolver objeto, nao lista.
    let Some(inicio) = out.find(['[', '{']) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(out[inicio..].trim()) else {
        return Vec::new();
    };
    let itens = match &v {
        serde_json::Value::Array(a) => a.clone(),
        outro => vec![outro.clone()],
    };
    itens
        .iter()
        .filter_map(|p| {
            let pid = p.get("Id")?.as_u64()? as u32;
            let name = p.get("ProcessName")?.as_str()?.to_string();
            // CPU vem `null` para processo sem tempo contabilizado.
            let cpu = p.get("CPU").and_then(|c| c.as_f64()).unwrap_or(0.0);
            let ws = p.get("WorkingSet64").and_then(|w| w.as_f64()).unwrap_or(0.0);
            Some(Proc {
                pid,
                name,
                cpu: (cpu * 10.0).round() / 10.0,
                mem_mb: (ws / 1_048_576.0 * 10.0).round() / 10.0,
            })
        })
        .collect()
}

/// Encerra um processo. `force` troca o pedido educado pelo definitivo.
///
/// Exige `confirm` pelo mesmo motivo do `/api/focus` e do `set-host`:
/// matar processo na maquina de alguem nao pode acontecer por clique
/// distraido, e nao tem desfazer.
pub fn kill(
    cfg: &crate::config::Config,
    host: &str,
    pid: u32,
    force: bool,
    confirm: bool,
) -> Result<String, String> {
    if !confirm {
        return Err("falta confirm: encerrar processo nao tem desfazer".into());
    }
    if pid <= 1 {
        return Err("pid 0 e 1 sao do sistema; recusado".into());
    }
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' nao esta na config"))?;

    let cmd = match h.os() {
        HostOs::Windows => {
            // `/T` leva junto a arvore: fechar so o pai deixa filho orfao
            // rodando, e o usuario clicou em "encerrar" esperando que
            // aquilo sumisse.
            if force {
                format!("taskkill /PID {pid} /T /F")
            } else {
                format!("taskkill /PID {pid} /T")
            }
        }
        HostOs::Linux | HostOs::Macos => {
            let sinal = if force { "KILL" } else { "TERM" };
            format!("kill -{sinal} {pid}")
        }
        HostOs::Unknown => {
            return Err("SO do host desconhecido: declare `os` em [[hosts]]".into())
        }
    };
    let saida = remote::run(h, &cmd, TIMEOUT_S)?;
    let modo = if force { "forcado" } else { "educado" };
    say!("procs: encerrei pid {pid} em {host} ({modo})");
    Ok(if saida.trim().is_empty() { format!("pid {pid} encerrado ({modo})") } else { saida })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parseia_ps_do_unix() {
        let out = "\
 1234  12.5  65432 /usr/lib/firefox/firefox
   42   0.0   1024 kworker/0:1
  777   3.2 204800 /Applications/Visual Studio Code.app/Contents/MacOS/Electron
";
        let v = parse_ps(out);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].pid, 1234);
        assert_eq!(v[0].name, "firefox", "fica so o ultimo componente do caminho");
        assert_eq!(v[0].cpu, 12.5);
        assert_eq!(v[0].mem_mb, 63.9, "rss vem em KiB");
        // Caminho com espaco nao pode truncar o nome.
        assert_eq!(v[2].name, "Electron");
    }

    #[test]
    fn parseia_get_process_do_windows() {
        let out = r#"[{"Id":4321,"ProcessName":"chrome","CPU":87.421875,"WorkingSet64":524288000},
                      {"Id":9,"ProcessName":"idle","CPU":null,"WorkingSet64":8192}]"#;
        let v = parse_ps_json(out);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "chrome");
        assert_eq!(v[0].cpu, 87.4);
        assert_eq!(v[0].mem_mb, 500.0);
        // CPU nula nao pode derrubar a linha inteira.
        assert_eq!(v[1].cpu, 0.0);
    }

    #[test]
    fn objeto_unico_tambem_parseia() {
        // Com UM processo o ConvertTo-Json devolve objeto, nao lista.
        let v = parse_ps_json(r#"{"Id":1,"ProcessName":"so","CPU":1.0,"WorkingSet64":1048576}"#);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].mem_mb, 1.0);
    }
}
