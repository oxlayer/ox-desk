//! Inventario e troca de canal, agora por HID++ direto.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! Substituiu o backend via `solaar`, que era caro demais para o caminho
//! quente: `solaar show` custava 5,9 a 15,3 s e o `change_host` o chamava
//! 4x por dispositivo -- uma troca levava 105 s, e o usuario concluia que
//! tinha falhado. Por HID++ direto (crate `hidpp`, sobre hidapi) um ping
//! custa ~25 ms e a varredura completa ~1,6 s.
//!
//! A fronteira publica deste modulo nao mudou: `survey()` e
//! `change_host()` tem a mesma forma, e `focus.rs` nao soube da troca.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// Nome por slot, aprendido quando o dispositivo esta online.
///
/// Necessario porque o HID++ **nao le o nome de quem nao responde**: sem
/// cache, um dispositivo em outro canal simplesmente desaparecia da API e
/// a GUI ficava em branco -- regressao em relacao ao backend solaar, que
/// listava offline com nome.
/// O que lembramos de cada slot. Nome e obrigatorio; bateria e a ULTIMA
/// leitura, com quando foi -- porque HID++ nao le bateria de dispositivo
/// ausente, e "5% ha 2 min" vale mais que "?" numa bandeja que se olha de
/// relance.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Known {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_status: Option<String>,
    /// Epoch em segundos da leitura de bateria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_at: Option<u64>,
}

static SEEN: Mutex<Option<BTreeMap<u8, Known>>> = Mutex::new(None);

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Onde o cache vive entre execucoes. E runtime descartavel, nao config:
/// apagar nao perde nada alem de uma releitura.
fn cache_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/state")
        });
    base.join("logi").join("slots.json")
}

fn load_cache() -> BTreeMap<u8, Known> {
    let Ok(text) = std::fs::read_to_string(cache_path()) else {
        return BTreeMap::new();
    };
    // Formato novo.
    if let Ok(m) = serde_json::from_str::<BTreeMap<String, Known>>(&text) {
        return m
            .into_iter()
            .filter_map(|(k, v)| k.parse::<u8>().ok().map(|s| (s, v)))
            .collect();
    }
    // Formato antigo ({slot: nome}): migra sem perder o que ja sabiamos.
    serde_json::from_str::<BTreeMap<String, String>>(&text)
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| k.parse::<u8>().ok().map(|s| {
                    (s, Known { name: v, battery_percent: None, battery_status: None, battery_at: None })
                }))
                .collect()
        })
        .unwrap_or_default()
}

fn save_cache(m: &BTreeMap<u8, Known>) {
    let as_str: BTreeMap<String, Known> =
        m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let Ok(text) = serde_json::to_string(&as_str) else { return };
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, text);
}

fn with_cache<T>(f: impl FnOnce(&mut BTreeMap<u8, Known>) -> T) -> Option<T> {
    let mut guard = SEEN.lock().ok()?;
    if guard.is_none() {
        *guard = Some(load_cache());
    }
    let m = guard.as_mut()?;
    Some(f(m))
}

/// Guarda nome e, quando houver, a leitura de bateria.
fn remember(slot: u8, name: &str, bat: Option<(Option<u8>, Option<String>)>) {
    let changed = with_cache(|m| {
        let e = m.entry(slot).or_insert_with(|| Known {
            name: name.to_string(),
            battery_percent: None,
            battery_status: None,
            battery_at: None,
        });
        let mut dirty = false;
        if e.name != name {
            e.name = name.to_string();
            dirty = true;
        }
        if let Some((pct, lvl)) = bat {
            if e.battery_percent != pct || e.battery_status != lvl {
                dirty = true;
            }
            e.battery_percent = pct;
            e.battery_status = lvl;
            e.battery_at = Some(now_secs());
            dirty = true;
        }
        dirty
    });
    if changed == Some(true) {
        if let Some(snapshot) = with_cache(|m| m.clone()) {
            save_cache(&snapshot);
        }
    }
}

fn remembered(slot: u8) -> Option<String> {
    with_cache(|m| m.get(&slot).map(|k| k.name.clone())).flatten()
}

fn remembered_full(slot: u8) -> Option<Known> {
    with_cache(|m| m.get(&slot).cloned()).flatten()
}

/// Quantos slots do receptor varrer.
const MAX_SLOTS: u8 = 6;
/// De quantas em quantas varreduras fazemos a completa.
///
/// Varrer os 6 slots custa ~1 s so em timeout dos vazios. Com o cache
/// sabemos quais existem, e varremos so esses -- mas de vez em quando
/// precisamos olhar tudo, senao um dispositivo pareado depois nunca
/// apareceria.
const FULL_SCAN_EVERY: u32 = 10;
static SCANS: Mutex<u32> = Mutex::new(0);
/// Tentativas de confirmar que o dispositivo saiu, apos o ChangeHost.
const LEAVE_CHECKS: u8 = 8;
/// O dispositivo sai quase instantaneamente; 300 ms era espera morta.
const LEAVE_WAIT: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub slot: u8,
    pub name: String,
    /// Vazio: o HID++ nao reporta "kind" direto. Mantido por compat com a
    /// GUI, que ja exibia a coluna.
    pub kind: String,
    pub wpid: String,
    pub hidraw: Option<String>,
    /// `false` quando o dispositivo esta em outro canal Easy-Switch.
    /// Enquanto offline, **nenhum comando HID++ o alcanca**.
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping_ms: Option<f64>,
    /// Canal atual, 1-based.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_host: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_hosts: Option<u8>,
    /// Porcentagem, quando o dispositivo reporta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    /// Status DECLARADO pelo dispositivo -- nao confie cegamente: um MX
    /// Keys no cabo subiu de 5% para 50% dizendo "descarregando".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_next_level: Option<u8>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub charging: bool,
    /// Qual feature respondeu (0x1004 ou 0x1000), para diagnostico.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_source: Option<String>,
    /// Segundos desde a leitura de bateria. `0` = agora; valor alto
    /// significa que o dispositivo esta ausente e isto e memoria.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_age_s: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Receiver {
    pub name: String,
    pub hidraw: Option<String>,
    pub usb_id: Option<String>,
    pub serial: Option<String>,
    pub devices: Vec<Device>,
}

#[derive(Debug, Serialize)]
pub struct Survey {
    /// ACL de `uaccess` nos /dev/hidraw. Sem isso nada funciona.
    pub hidraw_access: bool,
    /// Mantido no JSON para nao quebrar a GUI; agora e a fonte real.
    pub backend: &'static str,
    pub receivers: Vec<Receiver>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Checa se conseguimos abrir algum /dev/hidraw.
///
/// A regra do Solaar concede por `TAG+="uaccess"`, que vira ACL para o
/// usuario logado. Pegadinha paga: se os nos nasceram ANTES de a regra
/// existir, nenhuma ACL foi aplicada e so `udevadm trigger` resolve.
pub fn hidraw_access() -> bool {
    let Ok(entries) = std::fs::read_dir("/dev") else { return false };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if n.starts_with("hidraw") && std::fs::File::open(e.path()).is_ok() {
            return true;
        }
    }
    false
}

/// Ping com retry.
///
/// Um ping perdido NAO significa ausencia -- dispositivo em economia de
/// energia, ou com bateria baixa, estoura os 250 ms e responde na
/// tentativa seguinte. O `survey` ja tinha retry; o `remap` e o
/// `change_host` nao tinham, e falhavam com "nao responde" segundos
/// depois de uma leitura bem-sucedida. Observado com os devices a 5%.
fn ping_retry(rx: &hidpp::Receiver, slot: u8, attempts: u8) -> Result<f64, String> {
    // Delega ao crate: uma implementacao de retry para os dois lados.
    if rx.present(slot, attempts) {
        rx.ping(slot)
    } else {
        Err("sem resposta apos retry".into())
    }
}

fn no_access() -> Survey {
    Survey {
        hidraw_access: false,
        backend: "hidpp",
        receivers: Vec::new(),
        error: Some(
            "sem acesso a /dev/hidraw: rode \
             `sudo udevadm trigger --subsystem-match=hidraw --action=add` \
             (a regra do Solaar so aplica ACL a nos criados depois dela)"
                .into(),
        ),
    }
}

/// Varredura completa, com nome e bateria.
pub fn survey() -> Survey {
    survey_opts(true)
}

/// Ultimo survey leve, para o caminho quente nao pagar ~360 ms.
static LAST: Mutex<Option<(std::time::Instant, Vec<(u8, String, bool, Option<u8>)>)>> =
    Mutex::new(None);

/// Dispositivos conhecidos (slot, nome, online, canal) do cache curto.
///
/// Devolve `None` se nao houver leitura recente -- quem chama decide se
/// vale pagar a varredura.
pub fn cached_devices(max_age: Duration) -> Option<Vec<(u8, String, bool, Option<u8>)>> {
    let g = LAST.lock().ok()?;
    let (at, v) = g.as_ref()?;
    (at.elapsed() <= max_age).then(|| v.clone())
}

fn store_last(devs: &[Device]) {
    if let Ok(mut g) = LAST.lock() {
        *g = Some((
            std::time::Instant::now(),
            devs.iter()
                .map(|d| (d.slot, d.name.clone(), d.online, d.current_host))
                .collect(),
        ));
    }
}

/// Survey do cache curto quando fresco; varre se estiver velho.
///
/// Correcao antes de velocidade: dado velho sobre quem detemos o
/// dispositivo levaria a tentar trocar o que nao temos.
pub fn survey_cached_or_fresh(max_age: Duration) -> Survey {
    if let Some(devs) = cached_devices(max_age) {
        return Survey {
            hidraw_access: true,
            backend: "hidpp (cache)",
            receivers: vec![Receiver {
                name: "Receptor Logitech".into(),
                hidraw: None,
                usb_id: None,
                serial: None,
                devices: devs
                    .into_iter()
                    .map(|(slot, name, online, host)| Device {
                        slot,
                        name,
                        kind: String::new(),
                        wpid: String::new(),
                        hidraw: None,
                        online,
                        ping_ms: None,
                        current_host: host,
                        num_hosts: None,
                        battery_percent: None,
                        battery_status: None,
                        battery_next_level: None,
                        charging: false,
                        battery_source: None,
                        battery_age_s: None,
                    })
                    .collect(),
            }],
            error: None,
        };
    }
    survey_opts(false)
}

/// Mantem o cache curto quente, para a borda nao esperar varredura.
pub fn spawn_refresher() {
    std::thread::spawn(|| loop {
        let _ = survey_opts(false);
        std::thread::sleep(Duration::from_millis(1500));
    });
}

/// `with_battery = false` para o caminho quente: a bateria custa duas
/// idas e voltas por dispositivo e nao influencia decisao de troca.
pub fn survey_opts(with_battery: bool) -> Survey {
    if !hidraw_access() {
        return no_access();
    }
    let api = match hidpp::hidapi::HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            return Survey {
                hidraw_access: true,
                backend: "hidpp",
                receivers: Vec::new(),
                error: Some(format!("hidapi: {e}")),
            }
        }
    };
    let rx = match hidpp::Receiver::open(&api) {
        Ok(r) => r,
        Err(e) => {
            return Survey {
                hidraw_access: true,
                backend: "hidpp",
                receivers: Vec::new(),
                error: Some(e),
            }
        }
    };

    // Um unico ping perdido NAO significa ausencia: dispositivo em
    // economia de energia pode estourar os 250 ms. Observado em uso real --
    // o teclado apareceu offline numa leitura e online na seguinte, estavel
    // em 26 ms. Retry so para slot com nome conhecido (ou seja, pareado):
    // slot vazio continua falhando rapido.
    // Nomes NAO sao relidos quando ja estao em cache: eram ~1,1 s dos
    // 1,6 s de uma varredura, gastos para descobrir o que ja sabiamos.
    let known: Vec<u8> = (1..=MAX_SLOTS).filter(|s| remembered(*s).is_some()).collect();
    let n = {
        let mut g = SCANS.lock().unwrap_or_else(|e| e.into_inner());
        *g = g.wrapping_add(1);
        *g
    };
    let full = known.is_empty() || n % FULL_SCAN_EVERY == 0;
    let slots: Vec<u8> = if full { (1..=MAX_SLOTS).collect() } else { known };
    let need_names = slots.iter().any(|s| remembered(*s).is_none());

    let mut raw: Vec<hidpp::Device> = slots
        .iter()
        .map(|&slot| match rx.ping(slot) {
            Ok(ms) => hidpp::Device {
                slot,
                name: if need_names { rx.device_name(slot).ok() } else { None },
                online: true,
                reply_ms: Some((ms * 10.0).round() / 10.0),
                host: rx.host_info(slot).ok(),
                battery: if with_battery { rx.battery(slot) } else { None },
            },
            Err(_) => hidpp::Device {
                slot,
                name: None,
                online: false,
                reply_ms: None,
                host: None,
                battery: None,
            },
        })
        .collect();
    for d in raw.iter_mut() {
        if d.online || remembered(d.slot).is_none() {
            continue;
        }
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(ms) = rx.ping(d.slot) {
            d.online = true;
            d.reply_ms = Some((ms * 10.0).round() / 10.0);
            d.host = rx.host_info(d.slot).ok();
            if d.name.is_none() {
                d.name = rx.device_name(d.slot).ok();
            }
        }
    }

    let devices: Vec<Device> = raw
        .into_iter()
        .filter_map(|d| {
            // Offline sem nome conhecido = slot provavelmente vazio: nao
            // poluimos a lista com os 6 slots do receptor.
            let name = d.name.clone().or_else(|| remembered(d.slot))?;
            // Gravar a bateria NAO pode depender de ter lido o nome agora:
            // com o nome em cache (o caso normal) nunca gravavamos, e o
            // arquivo ficava preso no formato antigo.
            let bat = d
                .battery
                .as_ref()
                .map(|b| (b.percent, b.status.map(String::from)));
            if bat.is_some() || d.name.is_some() {
                remember(d.slot, &name, bat);
            }
            Some((d, name))
        })
        .map(|(d, name)| {
            // Sem leitura agora? usa a ultima conhecida, declarando a idade.
            let cached = remembered_full(d.slot);
            let (pct, lvl, age) = match d.battery.as_ref() {
                Some(b) => (b.percent, b.status.map(String::from), Some(0)),
                None => (
                    cached.as_ref().and_then(|k| k.battery_percent),
                    cached.as_ref().and_then(|k| k.battery_status.clone()),
                    cached
                        .as_ref()
                        .and_then(|k| k.battery_at)
                        .map(|t| now_secs().saturating_sub(t)),
                ),
            };
            Device {
            slot: d.slot,
            battery_percent: pct,
            battery_status: lvl,
            battery_next_level: d.battery.as_ref().and_then(|b| b.next_level),
            charging: d.battery.as_ref().is_some_and(|b| b.charging),
            battery_source: d.battery.as_ref().map(|b| b.source.to_string()),
            battery_age_s: age,
            name,
            kind: String::new(),
            wpid: String::new(),
            hidraw: Some(rx.path.clone()),
            online: d.online,
            ping_ms: d.reply_ms,
            current_host: d.host.map(|h| h.current),
            num_hosts: d.host.map(|h| h.num_hosts),
            }
        })
        .collect();

    store_last(&devices);
    Survey {
        hidraw_access: true,
        backend: "hidpp",
        receivers: vec![Receiver {
            name: format!("Receptor Logitech {:04x}", rx.product_id),
            hidraw: Some(rx.path.clone()),
            usb_id: Some(format!("046d:{:04X}", rx.product_id)),
            serial: None,
            devices,
        }],
        error: None,
    }
}

/// Troca de canal SEM esperar confirmacao.
///
/// Para o caminho sentido pelo usuario: ele percebe a troca quando o
/// dispositivo de fato muda de radio, nao quando nos confirmamos. A
/// verificacao vira log em background.
///
/// Ainda pingamos uma vez antes -- e o unico jeito de saber que detemos o
/// dispositivo. Sem isso o `set_host` trata "sem resposta" como sucesso
/// (porque numa troca real o radio cai antes da resposta) e um no-op
/// pareceria ter funcionado.
pub fn change_host_now(slot: u8, host: u8) -> Result<(), String> {
    if host == 0 {
        return Err("canal e 1-based".into());
    }
    let api = hidpp::hidapi::HidApi::new().map_err(|e| format!("hidapi: {e}"))?;
    let rx = hidpp::Receiver::open(&api)?;
    rx.ping(slot)
        .map_err(|_| format!("slot {slot} nao responde -- nao detemos este dispositivo"))?;
    rx.set_host_nowait(slot, host)
}

/// Confirma, em background, que o dispositivo saiu. So loga.
pub fn verify_left(slot: u8, host: u8) {
    std::thread::spawn(move || {
        let Ok(api) = hidpp::hidapi::HidApi::new() else { return };
        let Ok(rx) = hidpp::Receiver::open(&api) else { return };
        for i in 1..=LEAVE_CHECKS {
            std::thread::sleep(LEAVE_WAIT);
            if !rx.present(slot, 1) {
                println!(
                    "verificacao: slot {slot} saiu (canal {host}) em {}ms",
                    i as u32 * LEAVE_WAIT.as_millis() as u32
                );
                return;
            }
        }
        eprintln!("verificacao: slot {slot} AINDA responde -- a troca para o canal {host} nao pegou");
    });
}

/// Manda o dispositivo do slot `slot` para o canal `host` (1-based).
///
/// IRREVERSIVEL DESTE LADO: depois que ele troca, este receptor nao o
/// alcanca mais. O pre-flight tem de rodar ANTES.
///
/// Verificamos pelo EFEITO -- o dispositivo parar de responder --, nao por
/// codigo de retorno. Com o solaar isso era obrigatorio porque ele travava
/// e mentia; por HID++ a ausencia de resposta ja e o caso esperado, e o
/// ping confirma em ~25 ms.
pub fn change_host(slot: u8, host: u8) -> Result<String, String> {
    if host == 0 {
        return Err("canal e 1-based".into());
    }
    let api = hidpp::hidapi::HidApi::new().map_err(|e| format!("hidapi: {e}"))?;
    let rx = hidpp::Receiver::open(&api)?;

    if ping_retry(&rx, slot, 3).is_err() {
        return Err(format!(
            "slot {slot} nao responde neste receptor (3 tentativas) -- HID++ \
             nao alcanca dispositivo em outro canal"
        ));
    }

    rx.set_host(slot, host)?;

    for attempt in 1..=LEAVE_CHECKS {
        std::thread::sleep(LEAVE_WAIT);
        if rx.ping(slot).is_err() {
            return Ok(format!(
                "slot {slot} saiu deste receptor (canal {host}), confirmado em {}ms",
                attempt as u32 * LEAVE_WAIT.as_millis() as u32
            ));
        }
    }
    Err(format!(
        "slot {slot} continua respondendo aqui apos o comando -- a troca \
         para o canal {host} nao aconteceu"
    ))
}


#[derive(Debug, Serialize)]
pub struct ControlInfo {
    pub cid: String,
    pub name: &'static str,
    pub reprogrammable: bool,
    pub divertable: bool,
    pub flags: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remapped_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remapped_to_name: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct DeviceControls {
    pub slot: u8,
    pub device: String,
    pub controls: Vec<ControlInfo>,
}

#[derive(Debug, Serialize)]
pub struct Controls {
    pub devices: Vec<DeviceControls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn open_rx<'a>(api: &'a hidpp::hidapi::HidApi) -> Result<hidpp::Receiver, String> {
    hidpp::Receiver::open(api)
}

/// Controles de cada dispositivo ONLINE.
///
/// Dispositivo ausente nao aparece: HID++ nao alcanca quem esta em outro
/// canal, e listar nome sem poder ler nem escrever seria enganoso.
pub fn controls() -> Controls {
    let api = match hidpp::hidapi::HidApi::new() {
        Ok(a) => a,
        Err(e) => return Controls { devices: vec![], error: Some(format!("hidapi: {e}")) },
    };
    let rx = match open_rx(&api) {
        Ok(r) => r,
        Err(e) => return Controls { devices: vec![], error: Some(e) },
    };

    let mut out = Vec::new();
    for slot in 1..=MAX_SLOTS {
        if ping_retry(&rx, slot, 2).is_err() {
            continue;
        }
        let name = remembered(slot)
            .or_else(|| rx.device_name(slot).ok())
            .unwrap_or_else(|| format!("slot {slot}"));
        match rx.controls(slot) {
            Ok(cs) => out.push(DeviceControls {
                slot,
                device: name,
                controls: cs
                    .into_iter()
                    .map(|c| ControlInfo {
                        cid: format!("0x{:04x}", c.cid),
                        name: hidpp::control_name(c.cid),
                        reprogrammable: c.reprogrammable,
                        divertable: c.divertable,
                        flags: format!("0x{:02x}", c.flags),
                        remapped_to: c.remapped_to.map(|t| format!("0x{t:04x}")),
                        remapped_to_name: c.remapped_to.map(hidpp::control_name),
                    })
                    .collect(),
            }),
            Err(e) => out.push(DeviceControls {
                slot,
                device: format!("{name} (erro: {e})"),
                controls: vec![],
            }),
        }
    }
    Controls { devices: out, error: None }
}

/// Aplica um remapeamento. `to = 0` volta ao padrao.
pub fn remap(slot: u8, cid: u16, to: u16) -> Result<String, String> {
    let api = hidpp::hidapi::HidApi::new().map_err(|e| format!("hidapi: {e}"))?;
    let rx = open_rx(&api)?;
    if ping_retry(&rx, slot, 4).is_err() {
        return Err(format!(
            "slot {slot} nao responde (4 tentativas) -- o dispositivo esta em \
             outro canal, ou com bateria baixa demais para responder"
        ));
    }
    // Restaurar o padrao e remapear o controle PARA ELE MESMO, nao para
    // zero. Medido: com `to = 0` a escrita retorna ok e o dispositivo
    // continua reportando o remapeamento antigo.
    let target = if to == 0 { cid } else { to };
    rx.set_control_remap(slot, cid, target)?;
    Ok(format!(
        "0x{cid:04x} ({}) -> {}",
        hidpp::control_name(cid),
        if target == cid {
            "padrao".to_string()
        } else {
            format!("0x{target:04x} ({})", hidpp::control_name(target))
        }
    ))
}
