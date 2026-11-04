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

pub mod protocol;

/// Reexportado para quem consome este crate nao precisar declarar a
/// dependencia de novo -- e para garantir a MESMA versao em todos.
pub use hidapi;

use hidapi::{HidApi, HidDevice};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Indice de feature por (slot, feature). E estavel por dispositivo, e
/// descobri-lo custa uma ida e volta -- no caminho quente isso conta.
static FEATURE_CACHE: Mutex<Option<BTreeMap<(u8, u16), u8>>> = Mutex::new(None);
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
/// Bateria moderna, em porcentagem. O MX Anywhere 3 tem esta.
/// Controles reprogramaveis. E por ele que se remapeia botao.
pub const FEATURE_REPROG_CONTROLS: u16 = 0x1B04;
pub const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
/// Bateria antiga, em faixas. O MX Keys tem esta -- por isso as duas.
pub const FEATURE_BATTERY_STATUS: u16 = 0x1000;

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

/// Bateria. Os dois features reportam de formas diferentes, e
/// dispositivos da mesma familia usam features diferentes -- medido: o
/// MX Anywhere 3 expoe 0x1004 (porcentagem) e o MX Keys 0x1000 (faixa).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    /// Porcentagem, quando o dispositivo reporta.
    pub percent: Option<u8>,
    /// Status **declarado pelo dispositivo**, nao deduzido por nos.
    ///
    /// Vale desconfiar dele: medido, um MX Keys no cabo subiu de 5% para
    /// 50% reportando `descarregando` o tempo todo -- e o `upower`, que le
    /// pelo driver do kernel, reporta o mesmo. A mentira e do firmware; o
    /// nome do campo deixa claro de quem e a afirmacao.
    pub status: Option<&'static str>,
    /// Proximo degrau de descarga que o dispositivo prevê (0x1000).
    pub next_level: Option<u8>,
    /// Carregando. Para 0x1004 usamos chargingStatus **ou** externalPower,
    /// que e mais confiavel que so o primeiro.
    pub charging: bool,
    /// Qual feature respondeu, para o diagnostico nao virar adivinhacao.
    pub source: &'static str,
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
    pub battery: Option<Battery>,
}

pub struct Receiver {
    dev: HidDevice,
    pub path: String,
    pub product_id: u16,
}

impl Receiver {
    /// Abre a colecao de report longo do primeiro receptor Logitech.
    /// Todos os receptores HID++ ligados, nao so o primeiro.
    ///
    /// Existe porque o usuario pos DOIS receptores na mesma maquina
    /// (levou o do Ubuntu para o Windows) e o `open` escolhia um,
    /// silenciosamente: o agente listava os slots de um receptor e
    /// reportava os dispositivos do outro como ausentes. O painel ficava
    /// coerente consigo mesmo e errado sobre o mundo.
    ///
    /// Ordem estavel pelo caminho do dispositivo, para duas leituras
    /// seguidas nao trocarem os receptores de lugar na lista.
    pub fn open_all(api: &HidApi) -> Vec<Self> {
        let mut caminhos: Vec<_> = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == LOGITECH_VID
                    && d.usage_page() == HIDPP_USAGE_PAGE
                    && d.usage() == USAGE_LONG
                    && (0xC500..=0xC5FF).contains(&d.product_id())
            })
            .map(|d| (d.path().to_string_lossy().to_string(), d.product_id()))
            .collect();
        caminhos.sort();
        caminhos
            .into_iter()
            .filter_map(|(path, product_id)| {
                let dev = api.open_path(&std::ffi::CString::new(path.clone()).ok()?).ok()?;
                dev.set_blocking_mode(false).ok()?;
                Some(Self { dev, path, product_id })
            })
            .collect()
    }

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
    ///
    /// Memoizado: o indice nao muda para um dado dispositivo, e no caminho
    /// quente cada ida e volta evitada e ~25 ms.
    pub fn feature_index(&self, slot: u8, feature: u16) -> Result<u8, String> {
        if let Ok(g) = FEATURE_CACHE.lock() {
            if let Some(m) = g.as_ref() {
                if let Some(&i) = m.get(&(slot, feature)) {
                    return Ok(i);
                }
            }
        }
        let idx = self.feature_index_uncached(slot, feature)?;
        if let Ok(mut g) = FEATURE_CACHE.lock() {
            g.get_or_insert_with(BTreeMap::new).insert((slot, feature), idx);
        }
        Ok(idx)
    }

    fn feature_index_uncached(&self, slot: u8, feature: u16) -> Result<u8, String> {
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

    /// O dispositivo esta presente? Com retry.
    ///
    /// Use ISTO para presenca, nao `host_info`: aquele custa duas idas e
    /// volta com timeout de 700 ms cada, e um unico ping perdido nao
    /// significa ausencia. Medido em uso real: com os dispositivos a 5% de
    /// bateria, a checagem sem retry reportou "nenhum dispositivo neste
    /// receptor" 25 vezes em 5 minutos enquanto o usuario movia o mouse
    /// naquela maquina -- e a borda nao funcionava.
    pub fn present(&self, slot: u8, attempts: u8) -> bool {
        for i in 0..attempts.max(1) {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(160));
            }
            if self.ping(slot).is_ok() {
                return true;
            }
        }
        false
    }

    /// Numero de hosts e canal atual (1-based).
    ///
    /// **Valida a faixa antes de devolver.** O 0x1814 responde lixo
    /// durante reconexao: o monitor registrou um MX Keys -- que tem tres
    /// hosts -- declarando `current_host = 5`. Numero impossivel, e
    /// devolve-lo como fato fazia tudo que le o canal decidir sobre
    /// ficcao. O reconciliador agia nele.
    ///
    /// Fora da faixa vira erro, nao numero: quem chama ja sabe lidar com
    /// "nao sei" (o `survey` poe `None`), e nao sabe lidar com um canal
    /// que nao existe.
    pub fn host_info(&self, slot: u8) -> Result<HostInfo, String> {
        let fi = self.feature_index(slot, FEATURE_CHANGE_HOST)?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0])?;
        if r.len() < 2 {
            return Err("resposta curta de getHostInfo".into());
        }
        let num_hosts = r[0];
        let current = r[1] + 1;
        // Um dispositivo sem host nenhum nao existe, e canal acima do
        // total que ele proprio declarou e resposta corrompida.
        if num_hosts == 0 || num_hosts > 16 || current == 0 || current > num_hosts {
            return Err(format!(
                "getHostInfo incoerente: canal {current} de {num_hosts} host(s)"
            ));
        }
        Ok(HostInfo { num_hosts, current })
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

    /// Bateria, tentando 0x1004 (porcentagem) e caindo para 0x1000 (faixa).
    pub fn battery(&self, slot: u8) -> Option<Battery> {
        // 0x1004 UNIFIED_BATTERY, func 1 = getStatus
        if let Ok(fi) = self.feature_index(slot, FEATURE_UNIFIED_BATTERY) {
            if let Ok((r, _)) = self.request(slot, fi, 0x01, &[0, 0, 0]) {
                if r.len() >= 3 {
                    // r[0]=stateOfCharge(%), r[1]=level, r[2]=chargingStatus,
                    // r[3]=externalPowerStatus. Medido no MX Anywhere 3 no
                    // cabo: 05 01 01 01.
                    let ext = r.get(3).copied().unwrap_or(0);
                    return Some(Battery {
                        percent: Some(r[0]),
                        status: Some(if r[2] != 0 || ext != 0 {
                            "carregando"
                        } else {
                            "descarregando"
                        }),
                        next_level: None,
                        charging: r[2] != 0 || ext != 0,
                        source: "0x1004",
                    });
                }
            }
        }
        // 0x1000 BATTERY_STATUS, func 0 = getBatteryLevelStatus
        let fi = self.feature_index(slot, FEATURE_BATTERY_STATUS).ok()?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0]).ok()?;
        if r.len() < 3 {
            return None;
        }
        // r[0]=nivel de descarga (%), r[1]=proximo degrau, r[2]=status.
        // Medido no MX Keys carregando: 32 14 00 -- 50%, proximo 20%, e
        // status 0 ("descarregando") apesar de estar no cabo.
        let status = match r[2] {
            0 => "descarregando",
            1 => "recarregando",
            2 => "quase cheia",
            3 => "cheia",
            4 => "carga lenta",
            5 => "invalida",
            6 => "erro",
            _ => "desconhecido",
        };
        Some(Battery {
            percent: if r[0] > 0 { Some(r[0]) } else { None },
            status: Some(status),
            next_level: if r[1] > 0 { Some(r[1]) } else { None },
            charging: matches!(r[2], 1 | 2),
            source: "0x1000",
        })
    }

    /// Troca de canal SEM esperar resposta.
    ///
    /// Numa troca bem-sucedida o dispositivo sai do radio **antes** de
    /// responder, entao esperar e esperar por nada: medido, o
    /// `REPLY_TIMEOUT` de 700 ms era pago inteiro em cada troca. Aqui
    /// escrevemos o report e voltamos.
    ///
    /// Com o indice do feature memoizado, isto custa uma escrita -- a
    /// ordem de 1 ms em vez de 700.
    pub fn set_host_nowait(&self, slot: u8, host: u8) -> Result<(), String> {
        if host == 0 {
            return Err("host e 1-based".into());
        }
        let fi = self.feature_index(slot, FEATURE_CHANGE_HOST)?;
        let head = (0x01u8 << 4) | SW_ID;
        let mut out = [0u8; LONG_LEN];
        out[0] = REPORT_LONG;
        out[1] = slot;
        out[2] = fi;
        out[3] = head;
        out[4] = host - 1;
        self.dev.write(&out).map(|_| ()).map_err(|e| format!("write: {e}"))
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

    /// Varre os slots 1..=max.
    ///
    /// `with_names` e `with_battery` sao opcionais porque **custam ida e
    /// volta**: o nome sao ~2 requisicoes e a bateria mais 2. Com o nome
    /// em cache do lado de quem chama, pedir de novo e desperdicio puro --
    /// era 1,1 s dos 1,6 s de uma varredura.
    pub fn devices(&self, max: u8, with_names: bool, with_battery: bool) -> Vec<Device> {
        (1..=max)
            .map(|slot| match self.ping(slot) {
                Ok(ms) => Device {
                    slot,
                    name: if with_names { self.device_name(slot).ok() } else { None },
                    online: true,
                    reply_ms: Some((ms * 10.0).round() / 10.0),
                    host: self.host_info(slot).ok(),
                    battery: if with_battery { self.battery(slot) } else { None },
                },
                Err(_) => Device {
                    slot,
                    name: None,
                    online: false,
                    reply_ms: None,
                    host: None,
                    battery: None,
                },
            })
            .collect()
    }
}

/// Nome legivel de um controlID. Tabela parcial, das familias MX.
///
/// Os numeros vem do hardware (0x1B04 func 1); os nomes conferem com o
/// que o Solaar exibia para os mesmos dispositivos.
pub fn control_name(cid: u16) -> &'static str {
    match cid {
        0x0050 => "Botao esquerdo",
        0x0051 => "Botao direito",
        0x0052 => "Botao do meio",
        0x0053 => "Voltar",
        0x0056 => "Avancar",
        0x005b => "Gesto",
        0x006e => "Mostrar area de trabalho",
        0x00c3 => "Roda horizontal",
        0x00c4 => "Smart Shift",
        0x00c7 => "Easy-Switch 1",
        0x00c8 => "Easy-Switch 2",
        0x00d1 => "Brilho -",
        0x00d2 => "Brilho +",
        0x00d3 => "Mission Control / Task View",
        0x00d7 => "Botao de gesto virtual",
        0x00e0 => "Easy-Switch 3",
        0x00e1 => "Backlight -",
        0x00e2 => "Backlight +",
        0x00e3 => "Faixa anterior",
        _ => "desconhecido",
    }
}

#[derive(Debug, Clone)]
pub struct Control {
    pub cid: u16,
    pub tid: u16,
    pub flags: u8,
    /// Pode receber outra funcao (flag 0x10).
    pub reprogrammable: bool,
    /// Pode ser desviado para software (flag 0x20).
    pub divertable: bool,
    /// Para que CID ele esta mapeado agora, se o dispositivo informar.
    pub remapped_to: Option<u16>,
}

impl Receiver {
    /// Lista os controles reprogramaveis do dispositivo.
    pub fn controls(&self, slot: u8) -> Result<Vec<Control>, String> {
        let fi = self.feature_index(slot, FEATURE_REPROG_CONTROLS)?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0])?;
        let count = *r.first().ok_or("getCount vazio")?;
        let mut out = Vec::new();
        for i in 0..count {
            let (c, _) = self.request(slot, fi, 0x01, &[i, 0, 0])?;
            if c.len() < 5 {
                continue;
            }
            let cid = u16::from_be_bytes([c[0], c[1]]);
            let tid = u16::from_be_bytes([c[2], c[3]]);
            let flags = c[4];
            // func 2 = getControlReporting. Nao todos respondem; ausencia
            // nao e erro, so significa "sem remapeamento declarado".
            let remapped_to = self
                .request(slot, fi, 0x02, &[c[0], c[1], 0])
                .ok()
                .and_then(|(rr, _)| {
                    (rr.len() >= 5).then(|| u16::from_be_bytes([rr[3], rr[4]]))
                })
                .filter(|&to| to != 0 && to != cid);
            out.push(Control {
                cid,
                tid,
                flags,
                reprogrammable: flags & 0x10 != 0,
                divertable: flags & 0x20 != 0,
                remapped_to,
            });
        }
        Ok(out)
    }

    /// Remapeia um controle para agir como outro.
    ///
    /// `to` = 0 devolve o comportamento padrao. Recusa controle que o
    /// proprio dispositivo diz nao ser reprogramavel -- o botao esquerdo,
    /// por exemplo, reporta flags=0x01 sem o bit 0x10.
    pub fn set_control_remap(&self, slot: u8, cid: u16, to: u16) -> Result<(), String> {
        let ctl = self
            .controls(slot)?
            .into_iter()
            .find(|c| c.cid == cid)
            .ok_or_else(|| format!("controle 0x{cid:04x} nao existe neste dispositivo"))?;
        if !ctl.reprogrammable {
            return Err(format!(
                "0x{cid:04x} ({}) nao e reprogramavel: o dispositivo reporta \
                 flags=0x{:02x}, sem o bit 0x10",
                control_name(cid),
                ctl.flags
            ));
        }
        let fi = self.feature_index(slot, FEATURE_REPROG_CONTROLS)?;
        // func 3 = setControlReporting(cid, flags, remap)
        // flags bit 1 (0x02) = aplicar o remap que vem a seguir.
        let p = [
            (cid >> 8) as u8,
            (cid & 0xFF) as u8,
            0x02,
            (to >> 8) as u8,
            (to & 0xFF) as u8,
        ];
        self.request(slot, fi, 0x03, &p).map(|_| ())
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

#[cfg(test)]
mod testes_host_info {
    /// A validacao de faixa, isolada da leitura do hardware.
    ///
    /// Mesma conta que o `host_info` faz depois de ler os dois bytes.
    /// Existe como funcao de teste porque o caminho real precisa de um
    /// receptor na mesa, e a regra -- "canal dentro do total que o
    /// dispositivo declarou" -- nao precisa.
    fn coerente(num_hosts: u8, current_zero_based: u8) -> bool {
        let current = current_zero_based + 1;
        !(num_hosts == 0 || num_hosts > 16 || current == 0 || current > num_hosts)
    }

    #[test]
    fn canal_dentro_da_faixa_passa() {
        assert!(coerente(3, 0), "host 1 de 3");
        assert!(coerente(3, 2), "host 3 de 3");
        assert!(coerente(1, 0), "unico host");
    }

    #[test]
    fn canal_acima_do_total_e_recusado() {
        // O caso real: MX Keys com tres hosts respondeu canal 5 durante
        // uma reconexao. Devolver isso como fato fazia o reconciliador
        // decidir sobre um canal que nao existe.
        assert!(!coerente(3, 4), "canal 5 de 3 hosts");
        assert!(!coerente(3, 3), "canal 4 de 3 hosts");
    }

    #[test]
    fn total_absurdo_e_recusado() {
        // `num_hosts` tambem vem da mesma resposta corrompida: um
        // dispositivo que declara 19 hosts leu lixo nos dois bytes.
        assert!(!coerente(0, 0), "zero hosts");
        assert!(!coerente(19, 3), "19 hosts");
    }
}
