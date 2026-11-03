//! HID++ 2.0 sobre HID cru -- o suficiente para inventariar dispositivos
//! Logitech e trocar canal Easy-Switch, em Linux e Windows.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! Substitui o backend via `solaar` no Linux. Medido: `solaar show` custa
//! **5,9 a 15,3 s**, e o `change_host` o chamava 4x por dispositivo -- uma
//! troca levava 105 s. Por HID++ direto, uma chamada custa **26 a 90 ms**.
//!
//! ## Protocolo, em resumo
//!
//! ```text
//! requisicao (report 0x11, 20 bytes):
//!   [0x11, device_index, feature_index, func<<4 | sw_id, p0..p16]
//! resposta: mesmo eco de device_index/feature_index/func
//! erro:     feature_index = 0xFF, codigo em byte 4
//! ```
//!
//! O feature 0x0000 (ROOT) esta SEMPRE no indice 0; por ele se descobre o
//! indice de qualquer outro, que varia por dispositivo.
//!
//! ## Qual endpoint abrir
//!
//! O receptor expoe uma colecao HID por tipo de report (usage 1 = curto
//! 0x10, 2 = longo 0x11, 4 = muito longo). Usamos SEMPRE o longo na
//! **usage 2**, para escrever e ler na mesma colecao -- a alternativa
//! obriga a abrir duas e correlacionar entre elas.
//!
//! No Linux cada dispositivo pareado tem no hidraw proprio, mas ele atende
//! no **indice de slot do receptor** (medido: /dev/hidraw2 responde em
//! idx=1), nao em 0xFF. Como o no do receptor atende todos os slots, e ele
//! que abrimos.

/// Reexportado para quem consome este crate nao precisar declarar a
/// dependencia de novo -- e para garantir a MESMA versao em todos.
pub use hidapi;

use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

pub const LOGITECH_VID: u16 = 0x046D;
/// Pagina de uso vendor-defined onde a Logitech expoe o HID++.
pub const HIDPP_USAGE_PAGE: u16 = 0xFF00;
/// Colecao do report longo (0x11).
const USAGE_LONG: u16 = 0x02;

const REPORT_LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
/// Identifica as NOSSAS requisicoes nas respostas, separando-as de
/// notificacao espontanea do dispositivo. 1..15; 0 e reservado.
const SW_ID: u8 = 0x0A;

pub const FEATURE_ROOT: u16 = 0x0000;
pub const FEATURE_DEVICE_NAME: u16 = 0x0005;
pub const FEATURE_CHANGE_HOST: u16 = 0x1814;

const REPLY_TIMEOUT: Duration = Duration::from_millis(700);
/// Timeout do ping de presenca. Medido: dispositivo presente responde em
/// 26 a 90 ms. 250 ms e folgado e deixa a varredura de 6 slots vazios em
/// ~1,5 s em vez de 4,4 s.
const PING_TIMEOUT: Duration = Duration::from_millis(250);
const READ_SLICE: i32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostInfo {
    pub num_hosts: u8,
    /// 1-based, como a config e o Solaar mostram.
    pub current: u8,
}

#[derive(Debug, Clone)]
pub struct Device {
    /// Slot no receptor, 1..6.
    pub slot: u8,
    /// Nome lido do proprio dispositivo (feature 0x0005). `None` se ele
    /// nao expoe a feature.
    pub name: Option<String>,
    pub online: bool,
    #[allow(dead_code)]
    pub reply_ms: Option<f64>,
    pub host: Option<HostInfo>,
}

pub struct Receiver {
    dev: HidDevice,
    pub path: String,
    pub product_id: u16,
}

impl Receiver {
    /// Abre a colecao de report longo do primeiro receptor Logitech.
    pub fn open(api: &HidApi) -> Result<Self, String> {
        let info = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == LOGITECH_VID
                    && d.usage_page() == HIDPP_USAGE_PAGE
                    && d.usage() == USAGE_LONG
            })
            // O receptor tem PID da familia C5xx; preferimos ele ao no de
            // um dispositivo individual, porque atende todos os slots.
            .min_by_key(|d| if (0xC500..=0xC5FF).contains(&d.product_id()) { 0 } else { 1 })
            .ok_or("nenhuma interface HID++ Logitech (vid 046d, usage page ff00, usage 2)")?;
        let path = info.path().to_string_lossy().to_string();
        let product_id = info.product_id();
        let dev = info
            .open_device(api)
            .map_err(|e| format!("abrir {path}: {e}"))?;
        dev.set_blocking_mode(false)
            .map_err(|e| format!("modo nao-bloqueante: {e}"))?;
        Ok(Self { dev, path, product_id })
    }

    fn request(
        &self,
        slot: u8,
        feature_index: u8,
        func: u8,
        params: &[u8],
    ) -> Result<(Vec<u8>, f64), String> {
        self.request_to(slot, feature_index, func, params, REPLY_TIMEOUT)
    }

    fn request_to(
        &self,
        slot: u8,
        feature_index: u8,
        func: u8,
        params: &[u8],
        timeout: Duration,
    ) -> Result<(Vec<u8>, f64), String> {
        let head = (func << 4) | SW_ID;
        let mut out = [0u8; LONG_LEN];
        out[0] = REPORT_LONG;
        out[1] = slot;
        out[2] = feature_index;
        out[3] = head;
        let n = params.len().min(LONG_LEN - 4);
        out[4..4 + n].copy_from_slice(&params[..n]);

        let t0 = Instant::now();
        self.dev.write(&out).map_err(|e| format!("write: {e}"))?;

        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 64];
        while Instant::now() < deadline {
            let got = self
                .dev
                .read_timeout(&mut buf, READ_SLICE)
                .map_err(|e| format!("read: {e}"))?;
            if got < 5 {
                continue;
            }
            let r = &buf[..got];
            if r[1] != slot {
                continue; // resposta de outro dispositivo no mesmo receptor
            }
            if r[2] == 0xFF {
                return Err(format!("erro HID++ 0x{:02x} (func 0x{func:02x})", r[4]));
            }
            if r[2] == feature_index && r[3] == head {
                return Ok((r[4..].to_vec(), t0.elapsed().as_secs_f64() * 1000.0));
            }
        }
        Err("sem resposta".into())
    }

    /// ROOT.getFeature -- indice do feature neste dispositivo.
    pub fn feature_index(&self, slot: u8, feature: u16) -> Result<u8, String> {
        let p = [(feature >> 8) as u8, (feature & 0xFF) as u8, 0];
        let (r, _) = self.request(slot, 0x00, 0x00, &p)?;
        let idx = *r.first().ok_or("resposta vazia de getFeature")?;
        if idx == 0 {
            return Err(format!("feature 0x{feature:04x} nao suportado"));
        }
        Ok(idx)
    }

    /// O dispositivo do slot responde? Devolve a latencia.
    ///
    /// Este e o teste de "online": ~26 a 90 ms, contra 5,9 s de um
    /// `solaar show`.
    pub fn ping(&self, slot: u8) -> Result<f64, String> {
        let p = [
            (FEATURE_CHANGE_HOST >> 8) as u8,
            (FEATURE_CHANGE_HOST & 0xFF) as u8,
            0,
        ];
        let (_, ms) = self.request_to(slot, 0x00, 0x00, &p, PING_TIMEOUT)?;
        Ok(ms)
    }

    /// Chamada crua, para sondar feature cujo layout ainda nao se conhece.
    ///
    /// Foi assim que decifrei o 0x1815 (HOSTS_INFO): imprimir os bytes e
    /// construir o parser sobre a resposta real, em vez de supor o layout.
    pub fn raw(
        &self,
        slot: u8,
        feature: u16,
        func: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, String> {
        let fi = self.feature_index(slot, feature)?;
        self.request(slot, fi, func, params).map(|(r, _)| r)
    }

    pub fn host_info(&self, slot: u8) -> Result<HostInfo, String> {
        let fi = self.feature_index(slot, FEATURE_CHANGE_HOST)?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0])?;
        if r.len() < 2 {
            return Err("resposta curta de getHostInfo".into());
        }
        Ok(HostInfo { num_hosts: r[0], current: r[1] + 1 })
    }

    /// Nome do dispositivo via feature 0x0005, lido em pedacos.
    ///
    /// Resolve por NOME em vez de numero de slot: slot e por receptor e
    /// **nao coincide entre maquinas** (medido -- no Windows o slot 2
    /// estava vazio enquanto aqui existe).
    pub fn device_name(&self, slot: u8) -> Result<String, String> {
        let fi = self.feature_index(slot, FEATURE_DEVICE_NAME)?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0])?;
        let len = *r.first().ok_or("getCount vazio")? as usize;
        let mut name = String::new();
        let mut at = 0usize;
        while at < len && at < 64 {
            let (chunk, _) = self.request(slot, fi, 0x01, &[at as u8, 0, 0])?;
            for &b in &chunk {
                if name.len() >= len {
                    break;
                }
                if b == 0 {
                    break;
                }
                name.push(b as char);
            }
            if chunk.is_empty() {
                break;
            }
            at += chunk.len().min(16);
        }
        let trimmed = name.trim().to_string();
        if trimmed.is_empty() {
            Err("nome vazio".into())
        } else {
            Ok(trimmed)
        }
    }

    /// CHANGE_HOST.setCurrentHost. `host` e 1-based.
    ///
    /// IRREVERSIVEL: o dispositivo sai deste receptor e nao responde mais
    /// aqui. **Nao esperar resposta** -- o radio cai antes dela.
    pub fn set_host(&self, slot: u8, host: u8) -> Result<(), String> {
        if host == 0 {
            return Err("host e 1-based".into());
        }
        let fi = self.feature_index(slot, FEATURE_CHANGE_HOST)?;
        match self.request(slot, fi, 0x01, &[host - 1, 0, 0]) {
            Ok(_) => Ok(()),
            // Ausencia de resposta e o caso ESPERADO aqui.
            Err(e) if e == "sem resposta" => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Varre os slots 1..=max, com nome e canal de quem responder.
    pub fn devices(&self, max: u8, with_names: bool) -> Vec<Device> {
        (1..=max)
            .map(|slot| match self.ping(slot) {
                Ok(ms) => Device {
                    slot,
                    name: if with_names { self.device_name(slot).ok() } else { None },
                    online: true,
                    reply_ms: Some((ms * 10.0).round() / 10.0),
                    host: self.host_info(slot).ok(),
                },
                Err(_) => Device {
                    slot,
                    name: None,
                    online: false,
                    reply_ms: None,
                    host: None,
                },
            })
            .collect()
    }
}

/// Endpoints HID++ visiveis, para diagnostico.
pub fn endpoints(api: &HidApi) -> Vec<(String, u16, u16, u16)> {
    api.device_list()
        .filter(|d| d.vendor_id() == LOGITECH_VID && d.usage_page() == HIDPP_USAGE_PAGE)
        .map(|d| {
            (
                d.path().to_string_lossy().to_string(),
                d.product_id(),
                d.usage_page(),
                d.usage(),
            )
        })
        .collect()
}
