//! HID++ 2.0 sobre HID cru -- o suficiente para CHANGE_HOST (0x1814).
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! Protocolo, em resumo:
//!   requisicao curta (report 0x10, 7 bytes):
//!     [0x10, device_index, feature_index, func<<4 | sw_id, p0, p1, p2]
//!   resposta longa (report 0x11, 20 bytes) ou curta, com o mesmo eco de
//!   device_index/feature_index/func.
//!
//! O feature 0x0000 (ROOT) esta SEMPRE no indice 0, e e por ele que se
//! descobre o indice de qualquer outro feature. Sem isso nao ha como
//! falar 0x1814, porque o indice varia por dispositivo.

use hidapi::{HidApi, HidDevice};
use std::time::Duration;

pub const LOGITECH_VID: u16 = 0x046D;
/// Pagina de uso vendor-defined onde a Logitech expoe o HID++.
pub const HIDPP_USAGE_PAGE: u16 = 0xFF00;

const REPORT_SHORT: u8 = 0x10;
const REPORT_LONG: u8 = 0x11;
/// Tamanho do report longo, com o report id: 1 + 19.
const LONG_LEN: usize = 20;
/// Usage da colecao que carrega o report longo (0x11) no receptor
/// Logitech. No Windows o receptor expoe uma colecao por tipo de report
/// (usage 1 = curto 0x10, 2 = longo 0x11, 4 = muito longo 0x12), e
/// escrever o tamanho errado na colecao errada faz o write falhar.
const USAGE_LONG: u16 = 0x02;

/// Software id: identifica as NOSSAS requisicoes nas respostas, para nao
/// confundir com notificacao espontanea do dispositivo. Qualquer valor
/// 1..15 serve; 0 e reservado.
const SW_ID: u8 = 0x0A;

pub const FEATURE_CHANGE_HOST: u16 = 0x1814;
/// HOSTS_INFO: nome e status de cada canal, guardados no dispositivo.
pub const FEATURE_HOSTS_INFO: u16 = 0x1815;


#[derive(Debug)]
pub struct HostInfo {
    pub num_hosts: u8,
    /// 0-based, como o protocolo devolve.
    pub current: u8,
}

pub struct Link {
    dev: HidDevice,
}

impl Link {
    /// Abre a colecao HID++ de report longo do receptor Logitech.
    ///
    /// Usamos SEMPRE o report longo (0x11, 20 bytes) mesmo para funcoes que
    /// caberiam no curto: assim escrevemos e lemos na MESMA colecao. A
    /// alternativa -- escrever curto na Col01 e ler a resposta longa na
    /// Col02 -- obriga a abrir duas e correlacionar entre elas.
    pub fn open(api: &HidApi, path: Option<&str>) -> Result<Self, String> {
        let candidates: Vec<_> = api
            .device_list()
            .filter(|d| d.vendor_id() == LOGITECH_VID && d.usage_page() == HIDPP_USAGE_PAGE)
            .collect();
        if candidates.is_empty() {
            return Err("nenhuma interface HID++ Logitech encontrada \
                        (vid 046d, usage page 0xff00)"
                .into());
        }
        let chosen = match path {
            Some(p) => candidates
                .into_iter()
                .find(|d| d.path().to_string_lossy() == p)
                .ok_or_else(|| format!("caminho nao encontrado: {p}"))?,
            None => candidates
                .iter()
                .find(|d| d.usage() == USAGE_LONG)
                .or_else(|| candidates.first())
                .copied()
                .unwrap(),
        };
        let dev = chosen
            .open_device(api)
            .map_err(|e| format!("abrir {:?}: {e}", chosen.path()))?;
        dev.set_blocking_mode(false)
            .map_err(|e| format!("modo nao-bloqueante: {e}"))?;
        Ok(Self { dev })
    }

    /// Envia requisicao curta e espera a resposta correspondente.
    fn request(
        &self,
        dev_index: u8,
        feature_index: u8,
        func: u8,
        params: [u8; 3],
    ) -> Result<Vec<u8>, String> {
        let head = (func << 4) | SW_ID;
        let mut out = [0u8; LONG_LEN];
        out[0] = REPORT_LONG;
        out[1] = dev_index;
        out[2] = feature_index;
        out[3] = head;
        out[4..7].copy_from_slice(&params);
        self.dev
            .write(&out)
            .map_err(|e| format!("write ({} bytes): {e}", out.len()))?;

        // Le descartando o que nao e nossa resposta: o dispositivo manda
        // notificacao espontanea (bateria, conexao) no mesmo canal.
        let deadline = std::time::Instant::now() + Duration::from_millis(1200);
        let mut buf = [0u8; 64];
        while std::time::Instant::now() < deadline {
            let n = self
                .dev
                .read_timeout(&mut buf, 150)
                .map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                continue;
            }
            let r = &buf[..n];
            if r.len() < 4 {
                continue;
            }
            if r[0] != REPORT_SHORT && r[0] != REPORT_LONG {
                continue;
            }
            if r[1] != dev_index {
                continue;
            }
            // Erro HID++ 2.0: feature_index 0xFF, com o codigo em r[4].
            if r[2] == 0xFF {
                return Err(format!("erro HID++ 0x{:02x} (func 0x{:02x})", r[4], func));
            }
            if r[2] == feature_index && r[3] == head {
                return Ok(r[4..].to_vec());
            }
        }
        Err(format!(
            "sem resposta para feature 0x{feature_index:02x} func 0x{func:02x} \
             (dispositivo offline ou em outro canal?)"
        ))
    }

    /// ROOT.getFeature -- descobre o indice de um feature neste dispositivo.
    pub fn feature_index(&self, dev_index: u8, feature: u16) -> Result<u8, String> {
        let p = [(feature >> 8) as u8, (feature & 0xFF) as u8, 0];
        let r = self.request(dev_index, 0x00, 0x00, p)?;
        let idx = *r.first().ok_or("resposta vazia de getFeature")?;
        if idx == 0 {
            return Err(format!("feature 0x{feature:04x} nao suportado"));
        }
        Ok(idx)
    }

    /// Chamada crua, para sondar feature cujo layout eu ainda nao confirmei.
    pub fn raw(
        &self,
        dev_index: u8,
        feature: u16,
        func: u8,
        params: [u8; 3],
    ) -> Result<Vec<u8>, String> {
        let fi = self.feature_index(dev_index, feature)?;
        self.request(dev_index, fi, func, params)
    }

    /// CHANGE_HOST.getHostInfo
    pub fn host_info(&self, dev_index: u8) -> Result<HostInfo, String> {
        let fi = self.feature_index(dev_index, FEATURE_CHANGE_HOST)?;
        let r = self.request(dev_index, fi, 0x00, [0, 0, 0])?;
        if r.len() < 2 {
            return Err("resposta curta de getHostInfo".into());
        }
        Ok(HostInfo { num_hosts: r[0], current: r[1] })
    }

    /// CHANGE_HOST.setCurrentHost -- `host` e 0-based.
    ///
    /// IRREVERSIVEL: depois disto o dispositivo sai deste receptor e nao
    /// responde mais aqui. Nao espere resposta -- o radio cai antes.
    pub fn set_host(&self, dev_index: u8, host: u8) -> Result<(), String> {
        let fi = self.feature_index(dev_index, FEATURE_CHANGE_HOST)?;
        match self.request(dev_index, fi, 0x01, [host, 0, 0]) {
            Ok(_) => Ok(()),
            // Sem resposta e o caso ESPERADO: o dispositivo troca de canal
            // e para de falar com este receptor no mesmo instante.
            Err(e) if e.starts_with("sem resposta") => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Lista interfaces HID++ visiveis, para diagnostico.
pub fn list(api: &HidApi) -> Vec<(String, u16, u16, i32)> {
    api.device_list()
        .filter(|d| d.vendor_id() == LOGITECH_VID && d.usage_page() == HIDPP_USAGE_PAGE)
        .map(|d| {
            (
                d.path().to_string_lossy().to_string(),
                d.product_id(),
                d.usage(),
                d.interface_number(),
            )
        })
        .collect()
}
