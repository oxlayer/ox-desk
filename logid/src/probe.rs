//! Sondagem: portas TCP, estado dos `units` e detalhe por SSH.
//!
//! O que `units` significa muda por plataforma -- unit do systemd no
//! Linux, servico ou processo no Windows. Ver `sys::unit_state`.

use crate::config::{Config, Expect, HostCfg, HostOs};
use serde::Serialize;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const TCP_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Serialize)]
pub struct ServiceState {
    pub label: String,
    pub port: u16,
    pub up: bool,
    pub ms: Option<f64>,
    /// Esta como deveria para ESTE papel (ver Expect).
    pub ok: bool,
    pub expect: Expect,
}

#[derive(Debug, Serialize)]
pub struct UnitState {
    pub unit: String,
    pub state: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Detail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_min: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_free_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_total_gb: Option<f64>,
    /// Uso de CPU, 0-100.
    ///
    /// A JANELA de amostragem difere por SO, e isso e deliberado: no
    /// Linux sao duas leituras do `/proc/stat` 200 ms apart (percentual
    /// exato), no macOS a soma do `%cpu` dos processos dividida pelos
    /// nucleos (media que decai em ~1 min) e no Windows o
    /// `LoadPercentage` do WMI (~1 s). Nenhuma e comparavel ao decimal
    /// com as outras -- todas respondem "esta maquina esta ocupada?",
    /// que e a pergunta que o cartao faz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
}

/// Estado de KVM **daquele** host: borda e reconciliador.
///
/// Existe porque o painel mostrava so os interruptores desta maquina, e
/// a borda e um comportamento de PAR: a do Windows apontada para ca
/// devolvia tudo a cada encostada enquanto esta tela dizia, com razao,
/// que a borda daqui estava desligada. O usuario nao tinha como ver isso
/// de lugar nenhum -- precisava abrir o painel da outra maquina.
#[derive(Debug, Default, Serialize)]
pub struct Kvm {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// As bordas ativas la, como "left -> ubuntu-host".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<String>,
    /// Por que nao sabemos. Melhor dizer que nao sabemos do que mostrar
    /// "desligado" sobre um host que nao respondeu.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HostState {
    pub name: String,
    pub addr: String,
    pub role: String,
    pub online: bool,
    pub services: Vec<ServiceState>,
    pub units: Vec<UnitState>,
    pub detail: Detail,
    pub kvm: Kvm,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub generated: String,
    pub hosts: Vec<HostState>,
}

/// Todos os enderecos de um nome, sem link-local, IPv4 na frente.
///
/// Devolve LISTA porque um nome pode ter familias que nao se comportam
/// igual. Medido: visto do Mac, `windows-host.local` resolve em IPv4 **e**
/// IPv6, o IPv4 conecta na 3389 e o IPv6 da timeout -- porque a regra de
/// firewall do Windows estava escrita como `192.168.1.0/24`, que e IPv4.
/// Pegando so o primeiro endereco, o painel do Mac dizia "RDP fechado"
/// sobre um servico que estava aberto.
///
/// Link-local sai por ser inutil sem o escopo certo (ver `resolve_for_ssh`).
fn resolve_all(addr: &str, port: u16) -> Vec<SocketAddr> {
    fn link_local(a: &SocketAddr) -> bool {
        match a {
            SocketAddr::V4(v) => v.ip().octets()[..2] == [169, 254],
            SocketAddr::V6(v) => v.ip().segments()[0] & 0xffc0 == 0xfe80,
        }
    }
    let mut all: Vec<SocketAddr> =
        (addr, port).to_socket_addrs().map(|it| it.collect()).unwrap_or_default();
    if all.is_empty() {
        all = crate::sys::resolve_fallback(addr, port).into_iter().collect();
    }
    all.retain(|a| !link_local(a));
    // IPv4 primeiro: nesta LAN e o caminho que sempre funciona, e tentar
    // o que funciona antes economiza o timeout do que nao funciona.
    all.sort_by_key(|a| !a.is_ipv4());
    all.truncate(3);
    all
}

/// Opcoes de ssh/sftp que contornam o resolvedor do PROPRIO ssh.
///
/// ## Por que isto existe
///
/// No Windows, o `ssh` lancado **pelo daemon** falha com "Could not
/// resolve hostname ubuntu-host" -- enquanto, no mesmo contexto de
/// tarefa agendada, `ping` resolve, `[System.Net.Dns]::GetHostAddresses`
/// resolve os dois nomes (curto e `.local`), e o mesmo `ssh` lancado por
/// um `cmd.exe` da mesma tarefa funciona. Ou seja: o resolvedor do SO
/// enxerga o nome; o do `ssh`, naquele processo, nao.
///
/// Isso ia doer calado: o teste manual passa e o daemon falha. E a mesma
/// familia do buraco de mDNS no Linux, onde `getaddrinfo` com AF_UNSPEC
/// desiste de `.local` e so `getent` resolve (ver `resolve`).
///
/// A saida e resolver **aqui**, onde funciona, e entregar o endereco
/// pronto. Nao fere a regra de nunca escrever IP (spec 007): a config
/// continua com nome, a resolucao acontece a cada chamada, e troca de IP
/// e absorvida. `HostKeyAlias` mantem a identidade da chave amarrada ao
/// NOME, nao ao endereco -- sem isso, cada IP novo pareceria um host
/// desconhecido.
pub fn ssh_addr_opts(addr: &str) -> Vec<String> {
    // Porta 22 so para a resolucao; nao conectamos aqui.
    let Some(sa) = resolve_for_ssh(addr, 22) else { return Vec::new() };
    vec![
        "-o".into(),
        format!("HostName={}", sa.ip()),
        "-o".into(),
        format!("HostKeyAlias={addr}"),
    ]
}

/// Endereco para entregar ao `ssh`: o primeiro de `resolve_all`.
///
/// O link-local foi o que fez o SFTP para o Mac morrer com "Connection
/// closed": visto do Windows, `mac-host.local` resolve **so em
/// IPv6**, e o primeiro endereco e `fe80::...%11`, que sem o escopo
/// correto nao leva a lugar nenhum. `None` significa deixar o `ssh`
/// tentar sozinho -- melhor que entregar endereco que nao conecta.
fn resolve_for_ssh(addr: &str, port: u16) -> Option<SocketAddr> {
    resolve_all(addr, port).into_iter().next()
}

/// (aberta, latencia_ms) -- tenta cada endereco ate um conectar.
pub fn tcp_probe(addr: &str, port: u16) -> (bool, Option<f64>) {
    let alvos = resolve_all(addr, port);
    if alvos.is_empty() {
        return (false, None);
    }
    // Divide o orcamento: com tres enderecos mortos, um timeout cheio por
    // endereco seria 4,5 s num unico servico do snapshot.
    let por_alvo = TCP_TIMEOUT / alvos.len() as u32;
    // Duas rodadas: pelo tailnet um pacote perdido fazia a maquina piscar
    // offline no painel, com ping e SSH de pe. Maquina morta custa o dobro.
    for _ in 0..2 {
        for sa in &alvos {
            let t0 = Instant::now();
            if TcpStream::connect_timeout(sa, por_alvo).is_ok() {
                return (true, Some(t0.elapsed().as_secs_f64() * 1000.0));
            }
        }
    }
    (false, None)
}

/// Uptime e RAM do par por SSH. Silencioso em qualquer falha: detalhe e
/// enfeite, nao pode derrubar o snapshot.
///
/// O comando e por SO. A primeira versao rodava PowerShell com CIM
/// sempre, porque o par era sempre Windows; contra Ubuntu ou macOS isso
/// falharia em silencio e o card ficaria sem detalhe para sempre, sem
/// nada dizendo por que.
///
/// Contrato de saida, igual nos tres: `<minutos>|<livre_gb>|<total_gb>`.
fn detail_cmd(os: HostOs) -> Option<&'static str> {
    Some(match os {
        HostOs::Windows => concat!(
            "$o=Get-CimInstance Win32_OperatingSystem;",
            "$c=Get-CimInstance Win32_ComputerSystem;",
            "$up=[int]((Get-Date)-$o.LastBootUpTime).TotalMinutes;",
            "$free=[math]::Round($o.FreePhysicalMemory/1MB,1);",
            "$tot=[math]::Round($c.TotalPhysicalMemory/1GB,1);",
            "$cpu=[int](Get-CimInstance Win32_Processor | ",
            "Measure-Object -Property LoadPercentage -Average).Average;",
            "\"$up|$free|$tot|$cpu\""
        ),
        // MemAvailable e nao MemFree: o kernel usa RAM livre como cache, e
        // MemFree num Linux saudavel e quase zero -- mostrar isso daria a
        // impressao de maquina afogada.
        //
        // Os valores sao guardados e impressos no END, e nao na hora em
        // que a linha aparece. Imprimir na hora amarrava a saida a ORDEM
        // do /proc/meminfo, onde `MemTotal` vem ANTES de `MemAvailable`:
        // os dois numeros sairam colados (`31.315.6`) e os dois campos
        // viravam `None`. O bug sobreviveu porque esta e a unica maquina
        // Linux e o detalhe dela nunca era consultado -- ninguem roda
        // isto contra si mesmo por SSH. Apareceu no instante em que o
        // cartao local passou a buscar detalhe.
        HostOs::Linux => concat!(
            "awk '{printf \"%d|\", $1/60}' /proc/uptime; ",
            "awk '/^MemAvailable:/{a=$2} /^MemTotal:/{t=$2} ",
            "END{printf \"%.1f|%.1f|\", a/1048576, t/1048576}' /proc/meminfo; ",
            // Duas amostras: o /proc/stat conta TEMPO acumulado desde o
            // boot, nao uso atual. Uma leitura so daria a media da vida
            // da maquina, que nunca muda e nao serve para nada.
            "s() { awk '/^cpu /{t=0;for(i=2;i<=NF;i++)t+=$i;print t\" \"($5+$6)}' /proc/stat; }; ",
            "a=$(s); sleep 0.2; b=$(s); ",
            "echo \"$a $b\" | awk '{d=$3-$1; i=$4-$2; ",
            "if(d>0) printf \"%.0f\", (d-i)*100/d; else printf \"0\"}'"
        ),
        // macOS nao tem /proc. `kern.boottime` da a epoca do boot, e as
        // paginas livres saem do vm_stat (free + inactive, que e o que de
        // fato esta disponivel).
        HostOs::Macos => concat!(
            // ANCORADO no inicio. A saida e
            //   `{ sec = 1793736815, usec = 137720 } Tue Nov  3 ...`
            // e um `.*sec = ` guloso casa ate o `usec = `, capturando
            // 137720 -- o uptime virava 56 anos, numero grande o
            // suficiente para parecer bug de unidade e nao de parse.
            "b=$(sysctl -n kern.boottime | sed -n 's/^{ sec = \\([0-9]*\\).*/\\1/p'); ",
            "up=$(( ( $(date +%s) - b ) / 60 )); ",
            "ps=$(sysctl -n hw.pagesize); ",
            "fr=$(vm_stat | awk -v p=$ps '/Pages free/{f=$3} /Pages inactive/{i=$3} ",
            "END{gsub(/\\./,\"\",f); gsub(/\\./,\"\",i); printf \"%.1f\", (f+i)*p/1073741824}'); ",
            "tot=$(echo $(sysctl -n hw.memsize) | awk '{printf \"%.1f\", $1/1073741824}'); ",
            // Sem `top -l 2`: ele espera ~1 s pela segunda amostra e o
            // cartao do Mac ficaria sempre um segundo atrasado. A soma do
            // %cpu dos processos dividida pelos nucleos ja responde "esta
            // ocupada?" sem custo nenhum.
            "n=$(sysctl -n hw.ncpu); ",
            "cpu=$(ps -A -o %cpu | awk -v n=$n 'NR>1{s+=$1} ",
            "END{v=s/n; if(v>100)v=100; printf \"%.0f\", v}'); ",
            "echo \"$up|$fr|$tot|$cpu\""
        ),
        HostOs::Unknown => return None,
    })
}

/// Uptime e RAM de um host -- inclusive DESTE.
///
/// Era `ssh_detail` e exigia `h.ssh`, entao a maquina local ficava sem:
/// ela nao tem SSH para si mesma. O cartao de quem estava olhando o
/// painel aparecia sem uptime nem RAM, que e justamente o cartao mais
/// facil de conferir -- e o usuario notou.
///
/// Quem resolve local x remoto e o `remote::run`, que ja existia para
/// isso. Um caminho para os dois casos: o comando e escolhido pelo SO do
/// ALVO, e rodar PowerShell contra o Ubuntu falharia calado.
pub fn detalhe(h: &HostCfg) -> Detail {
    let Some(cmd) = detail_cmd(h.os()) else {
        return Detail::default();
    };
    // Host remoto sem `ssh` na config nao tem por onde ser consultado.
    if !h.is_local() && h.ssh.is_none() {
        return Detail::default();
    }
    let Ok(text) = crate::remote::run(h, cmd, 3) else {
        return Detail::default();
    };
    let Some(line) = text.lines().filter(|l| l.contains('|')).next_back() else {
        return Detail::default();
    };
    let parts: Vec<&str> = line.trim().split('|').collect();
    // Tres campos ou mais: a CPU entrou depois, e aceitar a linha curta
    // faz um campo novo degradar em vez de apagar os que ja funcionavam.
    if parts.len() < 3 {
        return Detail::default();
    }
    Detail {
        uptime_min: parts[0].trim().parse::<f64>().ok().map(|v| v as u64),
        ram_free_gb: parts[1].trim().parse().ok(),
        ram_total_gb: parts[2].trim().parse().ok(),
        cpu_pct: parts
            .get(3)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|v| v.clamp(0.0, 100.0)),
    }
}

fn check_host(h: &HostCfg, cfg: &Config) -> HostState {
    let services: Vec<ServiceState> = std::thread::scope(|s| {
        let handles: Vec<_> = h
            .services
            .iter()
            .map(|svc| s.spawn(|| {
                let (up, ms) = tcp_probe(&h.addr, svc.port);
                let ok = match svc.expect {
                    Expect::Any => true,
                    Expect::Down => !up,
                    Expect::Up => up,
                };
                ServiceState {
                    label: svc.label.clone(),
                    port: svc.port,
                    up,
                    ms: ms.map(|v| (v * 10.0).round() / 10.0),
                    ok,
                    expect: svc.expect,
                }
            }))
            .collect();
        let mut v: Vec<ServiceState> = handles.into_iter().filter_map(|x| x.join().ok()).collect();
        v.sort_by_key(|s| s.port);
        v
    });

    // A maquina local esta online por construcao: este codigo esta
    // rodando NELA. Sem esta linha, um host sem nenhum servico listado
    // aparece "off" para sempre, porque a unica evidencia de vida era
    // "alguma porta respondeu". Foi o que aconteceu quando o KDE
    // Connect saiu da config: o card do Ubuntu ficou sem servicos e a
    // propria maquina que serve o painel apareceu desligada.
    //
    // Para host REMOTO sem servicos a resposta continua sendo "off", e
    // esta certa: nao temos evidencia nenhuma de que ele existe.
    let online = h.is_local() || services.iter().any(|s| s.up);
    // Units SO da maquina local. `unit_state` consulta o gerenciador
    // daqui: fazer isso por um host remoto devolveria o estado da maquina
    // errada com cara de verdade. Hoje nenhuma config remota lista units,
    // mas a primeira que listasse mentiria sem avisar.
    let units = if h.is_local() {
        h.units
            .iter()
            .map(|u| UnitState { unit: u.clone(), state: crate::sys::unit_state(u) })
            .collect()
    } else {
        h.units
            .iter()
            .map(|u| UnitState { unit: u.clone(), state: "remoto".into() })
            .collect()
    };
    // A propria maquina responde sempre; as outras, so quando estao no ar
    // -- consultar host desligado custa o timeout inteiro para devolver o
    // que ja sabemos.
    let detail = if h.is_local() || online { detalhe(h) } else { Detail::default() };

    // So quando esta no ar: uma chamada SSH para host desligado custa o
    // timeout inteiro e devolve o que ja sabemos.
    let kvm = if h.is_local() || online { kvm_state(h, cfg) } else { Kvm::default() };

    HostState {
        name: h.name.clone(),
        addr: h.addr.clone(),
        role: h.role.clone(),
        online,
        services,
        units,
        detail,
        kvm,
    }
}

/// Le borda e reconciliador de um host.
///
/// Local: direto da config em memoria, que e a verdade corrente.
/// Remoto: pelo `/api/layout` do daemon DELE, por SSH. O daemon escuta
/// so em 127.0.0.1 -- de proposito -- entao o tunel do SSH e o caminho,
/// e nao abrir a porta na LAN.
fn kvm_state(h: &HostCfg, cfg: &Config) -> Kvm {
    if h.is_local() {
        let b = &cfg.input.border;
        let f = &cfg.input.follow;
        return Kvm {
            border: Some(b.enabled),
            follow: Some(f.enabled),
            leader: f.leader.clone(),
            edges: cfg
                .border_edges()
                .into_iter()
                .map(|(e, t)| format!("{e} -> {t}"))
                .collect(),
            error: None,
        };
    }
    if h.ssh.is_none() {
        return Kvm::default();
    }
    // `curl` no Unix, `curl.exe` no Windows: la o shell do SSH e o
    // PowerShell, e `curl` e APELIDO de `Invoke-WebRequest`, que rejeita
    // a URL como argumento posicional. Medido -- o apelido engana.
    let prog = if crate::remote::is_powershell(h) { "curl.exe" } else { "curl" };
    let cmd = format!(
        "{prog} -s --max-time 4 http://127.0.0.1:{}/api/layout",
        cfg.port
    );
    let texto = match crate::remote::run(h, &cmd, 6) {
        Ok(t) => t,
        Err(e) => return Kvm { error: Some(e), ..Default::default() },
    };
    let Some(linha) = texto.lines().find(|l| l.trim_start().starts_with('{')) else {
        return Kvm { error: Some("o daemon de la nao respondeu JSON".into()), ..Default::default() };
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(linha) else {
        return Kvm { error: Some("JSON ilegivel".into()), ..Default::default() };
    };
    Kvm {
        border: v.get("border_enabled").and_then(|x| x.as_bool()),
        follow: v.get("follow_enabled").and_then(|x| x.as_bool()),
        leader: v.get("follow_leader").and_then(|x| x.as_str()).map(String::from),
        edges: v
            .get("edges")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter(|e| e.get("ativa").and_then(|x| x.as_bool()) != Some(false))
                    .filter_map(|e| {
                        Some(format!(
                            "{} -> {}",
                            e.get("edge")?.as_str()?,
                            e.get("target")?.as_str()?
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        error: None,
    }
}

pub fn snapshot(cfg: &Config) -> Snapshot {
    let hosts = std::thread::scope(|s| {
        let handles: Vec<_> =
            cfg.hosts.iter().map(|h| s.spawn(|| check_host(h, cfg))).collect();
        handles.into_iter().filter_map(|x| x.join().ok()).collect()
    });
    Snapshot { generated: crate::sys::hhmmss(), hosts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    /// Reimplementa o criterio do filtro para fixa-lo em teste -- a
    /// funcao real depende de resolucao de nome, que nao cabe em teste
    /// unitario.
    fn link_local(a: &SocketAddr) -> bool {
        match a {
            SocketAddr::V4(v) => v.ip().octets()[..2] == [169, 254],
            SocketAddr::V6(v) => v.ip().segments()[0] & 0xffc0 == 0xfe80,
        }
    }

    #[test]
    fn link_local_e_descartavel() {
        let v6_ll = SocketAddr::V6(SocketAddrV6::new(
            "fe80::cb7:7431:1bea:d882".parse::<Ipv6Addr>().unwrap(), 22, 0, 11));
        let v6_glob = SocketAddr::V6(SocketAddrV6::new(
            "2804:394:8b44:fe00:d1:763f:38e:9eb6".parse::<Ipv6Addr>().unwrap(), 22, 0, 0));
        let v4_ll = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(169, 254, 3, 7), 22));
        let v4 = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 147), 22));

        // O endereco que quebrou o SFTP para o Mac.
        assert!(link_local(&v6_ll), "fe80::/10 e link-local");
        assert!(link_local(&v4_ll), "169.254/16 e link-local");
        assert!(!link_local(&v6_glob), "IPv6 global serve -- medido com o Mac");
        assert!(!link_local(&v4));
    }
}
