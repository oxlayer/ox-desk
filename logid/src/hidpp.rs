//! HID++ -- inventario de dispositivos Logitech e troca de canal Easy-Switch.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! Fase 3a usa o Solaar como backend (parse de `solaar show`, escrita por
//! `solaar config <slot> change-host <n>`). Parsear CLI e fragil, e de
//! proposito temporario: a fronteira deste modulo foi desenhada para que
//! uma implementacao de HID++ 0x1814 sobre /dev/hidraw substitua o
//! backend sem tocar em quem chama. Ver a tabela de opcoes na spec 005.

use serde::Serialize;
use std::process::Command;
use std::time::Duration;

const SOLAAR_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    /// Slot no receptor (1..6). E o que o `solaar config` recebe.
    pub slot: u8,
    pub name: String,
    pub kind: String,
    pub wpid: String,
    pub hidraw: Option<String>,
    /// `false` quando o dispositivo esta em outro canal Easy-Switch.
    /// Enquanto offline, **nenhum comando HID++ o alcanca**.
    pub online: bool,
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
    /// ACL de `uaccess` presente nos /dev/hidraw. Sem isso nada funciona,
    /// e a falha do Solaar e obscura ("No supported device found").
    pub hidraw_access: bool,
    pub solaar: Option<String>,
    pub receivers: Vec<Receiver>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Checa se conseguimos abrir algum /dev/hidraw.
///
/// A regra do Solaar concede por `TAG+="uaccess"`, que vira ACL para o
/// usuario logado. Pegadinha real: se os nos nasceram ANTES de a regra
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

fn run_solaar(args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("solaar");
    cmd.args(args);
    // Sem timeout nativo em std::process; o Solaar pode travar esperando
    // dispositivo. O wrapper `timeout` do coreutils resolve sem crate.
    let out = Command::new("timeout")
        .arg(format!("{}", SOLAAR_TIMEOUT.as_secs()))
        .arg("solaar")
        .args(args)
        .output()
        .map_err(|e| format!("executar solaar: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() || !stdout.trim().is_empty() {
        Ok(stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .last()
            .unwrap_or("solaar falhou")
            .to_string())
    }
}

fn field(line: &str) -> Option<String> {
    line.split_once(':').map(|(_, v)| v.trim().to_string())
}

/// Parse de `solaar show`.
///
/// O formato: bloco do receptor, depois blocos `  N: Nome` indentados.
/// Dispositivo offline aparece com `Protocol : unknown (device is offline)`.
fn parse_show(text: &str) -> Vec<Receiver> {
    let mut receivers: Vec<Receiver> = Vec::new();
    let mut cur_dev: Option<Device> = None;

    for raw in text.lines() {
        let trimmed = raw.trim();
        let indent = raw.len() - raw.trim_start().len();

        // "  1: MX Anywhere 3" -> novo dispositivo.
        //
        // Indentacao EXATA de 2. Com dispositivo online o `solaar show`
        // tambem imprime features (9 espacos), firmware (17) e botoes, todos
        // no formato "N: texto" -- aceitar `indent >= 2` fazia o parser
        // engolir tudo isso como se fossem dispositivos. Bug real, que so
        // aparecia com device online.
        let is_device_header = indent == 2
            && trimmed
                .split_once(':')
                .is_some_and(|(k, _)| k.len() <= 2 && k.chars().all(|c| c.is_ascii_digit()));

        if is_device_header {
            if let (Some(d), Some(r)) = (cur_dev.take(), receivers.last_mut()) {
                r.devices.push(d);
            }
            let (slot, name) = trimmed.split_once(':').unwrap();
            cur_dev = Some(Device {
                slot: slot.trim().parse().unwrap_or(0),
                name: name.trim().to_string(),
                kind: String::new(),
                wpid: String::new(),
                hidraw: None,
                online: true, // ate prova em contrario
            });
            continue;
        }

        // Cabecalho de receptor: linha sem indentacao, nao vazia, nao versao
        if indent == 0 && !trimmed.is_empty() && !trimmed.starts_with("solaar version") {
            if let (Some(d), Some(r)) = (cur_dev.take(), receivers.last_mut()) {
                r.devices.push(d);
            }
            receivers.push(Receiver {
                name: trimmed.to_string(),
                hidraw: None,
                usb_id: None,
                serial: None,
                devices: Vec::new(),
            });
            continue;
        }

        let target_dev = cur_dev.is_some();
        if trimmed.starts_with("Device path") {
            let v = field(trimmed);
            if target_dev {
                cur_dev.as_mut().unwrap().hidraw = v;
            } else if let Some(r) = receivers.last_mut() {
                r.hidraw = v;
            }
        } else if trimmed.starts_with("USB id") {
            if let Some(r) = receivers.last_mut() {
                if !target_dev {
                    r.usb_id = field(trimmed);
                }
            }
        } else if trimmed.starts_with("Serial") && !target_dev {
            if let Some(r) = receivers.last_mut() {
                r.serial = field(trimmed);
            }
        } else if trimmed.starts_with("WPID") {
            if let Some(d) = cur_dev.as_mut() {
                d.wpid = field(trimmed).unwrap_or_default();
            }
        } else if trimmed.starts_with("Kind") {
            if let Some(d) = cur_dev.as_mut() {
                d.kind = field(trimmed).unwrap_or_default();
            }
        } else if trimmed.contains("device is offline") {
            if let Some(d) = cur_dev.as_mut() {
                d.online = false;
            }
        }
    }

    if let (Some(d), Some(r)) = (cur_dev, receivers.last_mut()) {
        r.devices.push(d);
    }
    receivers
}

pub fn survey() -> Survey {
    let access = hidraw_access();
    let version = run_solaar(&["--version"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if !access {
        return Survey {
            hidraw_access: false,
            solaar: version,
            receivers: Vec::new(),
            error: Some(
                "sem acesso a /dev/hidraw: rode \
                 `sudo udevadm trigger --subsystem-match=hidraw --action=add` \
                 (a regra do Solaar so aplica ACL a nos criados depois dela)"
                    .into(),
            ),
        };
    }

    match run_solaar(&["show"]) {
        Ok(text) => Survey {
            hidraw_access: true,
            solaar: version,
            receivers: parse_show(&text),
            error: None,
        },
        Err(e) => Survey {
            hidraw_access: true,
            solaar: version,
            receivers: Vec::new(),
            error: Some(e),
        },
    }
}

/// Manda o dispositivo do slot `slot` pular para o canal `host` (1-based).
///
/// IRREVERSIVEL DESTE LADO. Ver spec 005: o comando viaja por radio ate o
/// dispositivo; depois que ele troca de canal, este receptor nao o alcanca
/// mais. Quem chama tem que ter feito o pre-flight ANTES.
pub fn change_host(slot: u8, host: u8) -> Result<String, String> {
    if host == 0 {
        return Err("canal e 1-based".into());
    }
    let slot_s = slot.to_string();
    let host_s = host.to_string();
    run_solaar(&["config", &slot_s, "change-host", &host_s])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "solaar version 1.1.19-1

Unifying Receiver
  Device path  : /dev/hidraw0
  USB id       : 046d:C52B
  Serial       : 450D9DEA
  Has 2 paired device(s) out of a maximum of 6.

  1: MX Anywhere 3
     Device path  : /dev/hidraw2
     WPID         : 4090
     Kind         : mouse
     Protocol     : unknown (device is offline)
     Battery: unknown (device is offline).

  2: MX Keys Keyboard
     Device path  : /dev/hidraw3
     WPID         : 408A
     Kind         : keyboard
";

    #[test]
    fn parses_receiver_and_devices() {
        let rs = parse_show(SAMPLE);
        assert_eq!(rs.len(), 1, "um receptor");
        let r = &rs[0];
        assert_eq!(r.name, "Unifying Receiver");
        assert_eq!(r.hidraw.as_deref(), Some("/dev/hidraw0"));
        assert_eq!(r.usb_id.as_deref(), Some("046d:C52B"));
        assert_eq!(r.devices.len(), 2);
    }

    #[test]
    fn marks_offline_only_for_the_offline_device() {
        let rs = parse_show(SAMPLE);
        let d = &rs[0].devices;
        assert_eq!(d[0].slot, 1);
        assert_eq!(d[0].name, "MX Anywhere 3");
        assert_eq!(d[0].kind, "mouse");
        assert_eq!(d[0].wpid, "4090");
        assert!(!d[0].online, "o mouse esta offline no exemplo");
        assert_eq!(d[1].slot, 2);
        assert!(d[1].online, "o teclado nao tem linha de offline");
    }

    #[test]
    fn device_path_goes_to_the_device_not_the_receiver() {
        let rs = parse_show(SAMPLE);
        assert_eq!(rs[0].devices[0].hidraw.as_deref(), Some("/dev/hidraw2"));
        assert_eq!(rs[0].hidraw.as_deref(), Some("/dev/hidraw0"));
    }

    /// Saida real com dispositivo ONLINE: features, firmware e botoes
    /// aparecem como "N: texto" indentados. So os de indentacao 2 sao
    /// dispositivos.
    const SAMPLE_ONLINE: &str = "solaar version 1.1.19-1

Unifying Receiver
  Device path  : /dev/hidraw0
  Has 2 paired device(s) out of a maximum of 6.

  1: MX Anywhere 3
     Device path  : /dev/hidraw2
     WPID         : 4090
     Kind         : mouse
     Firmware: 
                 1: BL1 13.00.B0014
                 0: MPM 24.00.B0014
     Supports 32 HID++ 2.0 features:
         0: ROOT                   {0000} V0
        10: CHANGE HOST            {1814} V1
     Has 7 reprogrammable keys:
         0: Left Button               , default: Left Click
         5: Smart Shift               , default: Smart Shift

  2: MX Keys Keyboard
     WPID         : 408A
     Kind         : keyboard
     Supports 33 HID++ 2.0 features:
         9: CHANGE HOST            {1814} V1
        10: HOSTS INFO             {1815} V1
";

    #[test]
    fn ignores_features_firmware_and_buttons() {
        let rs = parse_show(SAMPLE_ONLINE);
        assert_eq!(rs.len(), 1);
        let d = &rs[0].devices;
        assert_eq!(d.len(), 2, "exatamente dois dispositivos, nao as features");
        assert_eq!(d[0].name, "MX Anywhere 3");
        assert_eq!(d[0].kind, "mouse");
        assert_eq!(d[1].name, "MX Keys Keyboard");
        assert_eq!(d[1].kind, "keyboard");
        assert!(d.iter().all(|x| x.online), "ambos online neste exemplo");
    }

    #[test]
    fn rejects_zero_channel() {
        assert!(change_host(1, 0).is_err());
    }
}
