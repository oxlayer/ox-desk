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
static SEEN: Mutex<Option<BTreeMap<u8, String>>> = Mutex::new(None);

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

fn load_cache() -> BTreeMap<u8, String> {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|t| serde_json::from_str::<BTreeMap<String, String>>(&t).ok())
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| k.parse::<u8>().ok().map(|s| (s, v)))
                .collect()
        })
        .unwrap_or_default()
}

fn save_cache(m: &BTreeMap<u8, String>) {
    let as_str: BTreeMap<String, String> =
        m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let Ok(text) = serde_json::to_string(&as_str) else { return };
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, text);
}

fn with_cache<T>(f: impl FnOnce(&mut BTreeMap<u8, String>) -> T) -> Option<T> {
    let mut guard = SEEN.lock().ok()?;
    if guard.is_none() {
        *guard = Some(load_cache());
    }
    let m = guard.as_mut()?;
    Some(f(m))
}

fn remember(slot: u8, name: &str) {
    let changed = with_cache(|m| {
        if m.get(&slot).map(String::as_str) == Some(name) {
            false
        } else {
            m.insert(slot, name.to_string());
            true
        }
    });
    if changed == Some(true) {
        if let Some(snapshot) = with_cache(|m| m.clone()) {
            save_cache(&snapshot);
        }
    }
}

fn remembered(slot: u8) -> Option<String> {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_level: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub charging: bool,
    /// Qual feature respondeu (0x1004 ou 0x1000), para diagnostico.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_source: Option<String>,
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
            if let Some(n) = d.name.as_deref() {
                remember(d.slot, n);
            }
            // Offline sem nome conhecido = slot provavelmente vazio: nao
            // poluimos a lista com os 6 slots do receptor.
            let name = d.name.clone().or_else(|| remembered(d.slot))?;
            Some((d, name))
        })
        .map(|(d, name)| Device {
            slot: d.slot,
            battery_percent: d.battery.as_ref().and_then(|b| b.percent),
            battery_level: d.battery.as_ref().and_then(|b| b.level).map(String::from),
            charging: d.battery.as_ref().is_some_and(|b| b.charging),
            battery_source: d.battery.as_ref().map(|b| b.source.to_string()),
            name,
            kind: String::new(),
            wpid: String::new(),
            hidraw: Some(rx.path.clone()),
            online: d.online,
            ping_ms: d.reply_ms,
            current_host: d.host.map(|h| h.current),
            num_hosts: d.host.map(|h| h.num_hosts),
        })
        .collect();

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

    if rx.ping(slot).is_err() {
        return Err(format!(
            "slot {slot} nao responde neste receptor -- HID++ nao alcanca \
             dispositivo em outro canal"
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
