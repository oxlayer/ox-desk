//! Captura de teclado e mouse por evdev, para o KVM por LAN.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! ## Por que evdev e nao o InputCapture do Mutter
//!
//! O GNOME 50 expoe `org.gnome.Mutter.InputCapture`, que e a API feita
//! exatamente para isto. Introspeccionando a sessao, ela oferece
//! `AddBarrier`, `Enable`, `Disable`, `ConnectToEIS` e o sinal
//! `Activated(barrier_id, ...)` -- e **nenhum metodo para ativar a
//! captura sob demanda**. O compositor so entrega o input quando o
//! ponteiro cruza uma barreira de borda.
//!
//! Isso e o oposto do que foi pedido: trocar de maquina por clique na
//! bandeja, sem depender de borda. Daria para contornar criando uma
//! barreira e teleportando o ponteiro para dentro dela pelo
//! `RemoteDesktop` -- mas seria simular a borda que estamos tentando
//! abandonar, com mais pecas para quebrar.
//!
//! O evdev resolve direto:
//!
//! - **Agarrar e soltar e explicito** (`EVIOCGRAB`), que e a semantica
//!   do clique na bandeja.
//! - **Os codigos ja sao os nossos.** `Event::Key` carrega keycode de
//!   evdev porque e o que o `NotifyKeyboardKeycode` do Mutter recebe, e
//!   `scancode_from_evdev()` ja traduz para o Windows. Nao ha conversao
//!   nova no caminho.
//! - **Independe de compositor.** Mesmo code em qualquer Linux.
//!
//! ## O que EVIOCGRAB faz, e o que ele custa
//!
//! Agarrado, o dispositivo para de entregar ao compositor: o teclado
//! simplesmente nao existe para o desktop local. E isso que queremos ao
//! mandar o input para outra maquina -- sem o grab, cada tecla seria
//! digitada nos DOIS computadores.
//!
//! O custo e a responsabilidade: **se este processo morrer agarrando, o
//! kernel solta** (o fd fecha), entao nao da para travar o teclado para
//! sempre. Mas enquanto estiver agarrado e preciso existir um caminho de
//! volta que nao dependa do teclado -- ver `TECLA_PANICO`.
//!
//! ## Permissao
//!
//! Ler `/dev/input/event*` exige ACL. Ver `logid/udev/70-logid-input.rules`:
//! usamos `TAG+="uaccess"` nos dispositivos Logitech, nao o grupo
//! `input`, porque o grupo daria a qualquer processo da conta acesso
//! permanente a todo dispositivo de entrada -- inclusive sem sessao
//! aberta.

#![cfg(target_os = "linux")]

use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::input::{Event, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};

/// `_IOW('E', 0x90, int)`.
const EVIOCGRAB: libc::c_ulong = 0x4004_4590;

// Tipos de evento do evdev que nos interessam.
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;

// Eixos relativos.
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;

// Botoes de mouse, na numeracao do evdev.
const EV_BTN_LEFT: u16 = 0x110;
const EV_BTN_RIGHT: u16 = 0x111;
const EV_BTN_MIDDLE: u16 = 0x112;

/// Um passo de roda em "pixels" de rolagem, como o resto do protocolo usa.
const PASSO_RODA: f64 = 120.0;

/// Um `struct input_event` de 64 bits: 2x i64 de tempo, u16, u16, i32.
const TAM_EVENTO: usize = 24;

/// Event cru do kernel, ja desempacotado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub tipo: u16,
    pub code: u16,
    pub valor: i32,
}

/// Traduz um lote de eventos crus (ate o `SYN_REPORT`) para os nossos.
///
/// Funcao pura: e aqui que mora a decisao, e e isto que os testes olham.
///
/// O evdev entrega movimento em eixos SEPARADOS -- `REL_X` numa linha,
/// `REL_Y` noutra -- e so o `SYN_REPORT` diz que o conjunto acabou.
/// Mandar um evento de rede por eixo dobraria o trafego e faria o cursor
/// andar em escada no outro lado, porque cada metade chegaria sozinha.
/// Por isso os eixos somam dentro do lote e saem como UM `MouseRel`.
/// `repetir`: repassar a auto-repeticao do kernel como novos
/// key-downs. Ver a nota sobre `valor == 2` dentro da funcao.
pub fn translate(lote: &[RawEvent], repetir: bool) -> Vec<Event> {
    let mut saida = Vec::new();
    let (mut dx, mut dy) = (0.0f64, 0.0f64);
    let (mut rx, mut ry) = (0.0f64, 0.0f64);

    for e in lote {
        match e.tipo {
            EV_REL => match e.code {
                REL_X => dx += f64::from(e.valor),
                REL_Y => dy += f64::from(e.valor),
                // Sinal invertido: no evdev, roda para CIMA e positivo;
                // no nosso protocolo (e no Wayland) para baixo e
                // positivo, porque o conteudo desce.
                REL_WHEEL => ry -= f64::from(e.valor) * PASSO_RODA,
                REL_HWHEEL => rx += f64::from(e.valor) * PASSO_RODA,
                _ => {}
            },
            EV_KEY => {
                // valor 2 = auto-repeticao gerada pelo kernel.
                //
                // A primeira versao descartava, com o comentario "o
                // outro lado repete sozinho a partir do key-down". Isso
                // e FALSO no Windows: a repeticao de tecla la nasce no
                // driver do teclado fisico, e nao se aplica a uma tecla
                // sintetica do `SendInput`. Resultado: segurar backspace
                // apagava um caractere so -- foi assim que apareceu.
                //
                // Repassar como key-down replica exatamente o que o
                // teclado fisico faz, que e o trabalho de um KVM.
                //
                // Fica atras de `repetir` porque um alvo que TAMBEM
                // repete sozinho (o Mutter aplica a propria logica a
                // teclas injetadas) receberia repeticao dobrada. Quem
                // sabe o SO do destino e quem lanca o processo.
                if e.valor == 2 && !repetir {
                    continue;
                }
                let pressed = e.valor != 0;
                match e.code {
                    EV_BTN_LEFT => saida.push(Event::Button { code: BTN_LEFT, pressed }),
                    EV_BTN_RIGHT => saida.push(Event::Button { code: BTN_RIGHT, pressed }),
                    EV_BTN_MIDDLE => saida.push(Event::Button { code: BTN_MIDDLE, pressed }),
                    cod => saida.push(Event::Key { keycode: u32::from(cod), pressed }),
                }
            }
            _ => {}
        }
    }

    // Movimento antes dos cliques do MESMO lote seria errado pelo
    // motivo oposto: um clique vem com a posicao onde o lote comecou.
    // Mas dentro de um lote o kernel nao mistura clique e movimento em
    // ordem significativa, entao a ordem aqui e estavel: move, depois
    // rola. Os botoes ja sairam na ordem em que chegaram.
    if dx != 0.0 || dy != 0.0 {
        saida.insert(0, Event::MouseRel { dx, dy });
    }
    if rx != 0.0 || ry != 0.0 {
        saida.push(Event::Scroll { dx: rx, dy: ry });
    }
    saida
}

/// Um `/dev/input/eventN` aberto.
pub struct Device {
    arquivo: File,
    pub nome: String,
    pub caminho: PathBuf,
    agarrado: bool,
    lote: Vec<RawEvent>,
    /// Repassar a auto-repeticao do kernel. Ver `translate`.
    repetir: bool,
}

impl Device {
    pub fn open(caminho: &Path) -> std::io::Result<Self> {
        let arquivo = File::open(caminho)?;
        let nome = name_of(caminho).unwrap_or_else(|| "?".into());
        Ok(Device {
            arquivo,
            nome,
            caminho: caminho.to_path_buf(),
            agarrado: false,
            lote: Vec::new(),
            repetir: true,
        })
    }

    /// Todos os `/dev/input/event*` que conseguimos abrir.
    ///
    /// Os que recusam acesso sao PULADOS em silencio de proposito: a
    /// maquina tem dezenas de nos de input (botao de energia, webcam,
    /// alto-falante) e nenhum deles nos interessa. Quem reclama de
    /// permissao e `exigir()`, com o nome do dispositivo que faltou.
    pub fn list() -> Vec<Device> {
        let mut v = Vec::new();
        let Ok(dir) = std::fs::read_dir("/dev/input") else {
            return v;
        };
        let mut caminhos: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("event"))
            })
            .collect();
        caminhos.sort();
        for c in caminhos {
            if let Ok(d) = Device::open(&c) {
                v.push(d);
            }
        }
        v
    }

    /// O dispositivo para de existir para o desktop local.
    pub fn grab(&mut self) -> std::io::Result<()> {
        if self.agarrado {
            return Ok(());
        }
        let r = unsafe { libc::ioctl(self.arquivo.as_raw_fd(), EVIOCGRAB, 1 as libc::c_int) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.agarrado = true;
        Ok(())
    }

    pub fn release(&mut self) -> std::io::Result<()> {
        if !self.agarrado {
            return Ok(());
        }
        let r = unsafe { libc::ioctl(self.arquivo.as_raw_fd(), EVIOCGRAB, 0 as libc::c_int) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.agarrado = false;
        Ok(())
    }

    /// Liga ou desliga o repasse da auto-repeticao do kernel.
    pub fn set_repeat(&mut self, r: bool) {
        self.repetir = r;
    }

    pub fn is_grabbed(&self) -> bool {
        self.agarrado
    }

    /// Le o que houver e devolve os eventos dos lotes COMPLETOS.
    ///
    /// Um lote so fecha no `SYN_REPORT`. Eventos de um lote ainda aberto
    /// ficam guardados para a proxima chamada -- entregar meio lote
    /// mandaria o eixo X sem o Y.
    pub fn read(&mut self) -> std::io::Result<Vec<Event>> {
        let mut buf = [0u8; TAM_EVENTO * 64];
        let n = self.arquivo.read(&mut buf)?;
        let mut saida = Vec::new();
        for pedaco in buf[..n].chunks_exact(TAM_EVENTO) {
            let tipo = u16::from_ne_bytes([pedaco[16], pedaco[17]]);
            let code = u16::from_ne_bytes([pedaco[18], pedaco[19]]);
            let valor = i32::from_ne_bytes([pedaco[20], pedaco[21], pedaco[22], pedaco[23]]);
            if tipo == EV_SYN {
                saida.extend(translate(&self.lote, self.repetir));
                self.lote.clear();
            } else {
                self.lote.push(RawEvent { tipo, code, valor });
            }
        }
        Ok(saida)
    }
}

impl Drop for Device {
    /// Soltar no `Drop` e cinto de seguranca, nao o mecanismo.
    ///
    /// O kernel ja solta o grab quando o fd fecha, entao teclado travado
    /// para sempre nao e um estado alcancavel nem se o processo for
    /// morto com SIGKILL. Isto so torna a ordem explicita para quem le.
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Nome do dispositivo, pelo sysfs.
///
/// Pelo sysfs e nao pelo ioctl `EVIOCGNAME`: o arquivo do sysfs pode ser
/// lido sem abrir o no de input, entao da para listar e escolher ANTES
/// de ter permissao -- que e o que permite dizer "falta ACL no MX Keys"
/// em vez de "nenhum dispositivo encontrado".
pub fn name_of(caminho: &Path) -> Option<String> {
    let base = caminho.file_name()?.to_str()?;
    let p = format!("/sys/class/input/{base}/device/name");
    Some(std::fs::read_to_string(p).ok()?.trim().to_string())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn rel(code: u16, valor: i32) -> RawEvent {
        RawEvent { tipo: EV_REL, code, valor }
    }
    fn tecla(code: u16, valor: i32) -> RawEvent {
        RawEvent { tipo: EV_KEY, code, valor }
    }

    #[test]
    fn eixos_do_mesmo_lote_viram_um_movimento_so() {
        let saida = translate(&[rel(REL_X, 5), rel(REL_Y, -3)], true);
        assert_eq!(saida, vec![Event::MouseRel { dx: 5.0, dy: -3.0 }]);
    }

    #[test]
    fn lote_sem_movimento_nao_manda_movimento() {
        let saida = translate(&[tecla(30, 1)], true);
        assert_eq!(saida, vec![Event::Key { keycode: 30, pressed: true }]);
    }

    /// Segurar uma tecla tem de apagar varios caracteres.
    ///
    /// O Windows nao repete tecla sintetica -- a repeticao de la nasce
    /// no driver do teclado fisico. Descartar o `valor == 2` do kernel
    /// fazia segurar backspace apagar UM caractere so, que foi como o
    /// usuario encontrou o bug.
    #[test]
    fn repeticao_do_kernel_e_repassada() {
        let saida = translate(&[tecla(14, 1), tecla(14, 2), tecla(14, 2), tecla(14, 0)], true);
        assert_eq!(
            saida,
            vec![
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: false },
            ]
        );
    }

    /// Alvo que repete sozinho recebe so o comeco e o fim.
    #[test]
    fn com_repeticao_desligada_so_passam_press_e_release() {
        let saida = translate(&[tecla(14, 1), tecla(14, 2), tecla(14, 2), tecla(14, 0)], false);
        assert_eq!(
            saida,
            vec![
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: false },
            ]
        );
    }

    #[test]
    fn botoes_viram_botao_e_nao_tecla() {
        let saida = translate(&[tecla(EV_BTN_LEFT, 1), tecla(EV_BTN_RIGHT, 0)], true);
        assert_eq!(
            saida,
            vec![
                Event::Button { code: BTN_LEFT, pressed: true },
                Event::Button { code: BTN_RIGHT, pressed: false },
            ]
        );
    }

    #[test]
    fn roda_para_cima_rola_para_cima() {
        // No evdev, roda para cima e +1. No nosso protocolo (e no
        // Wayland) o eixo Y cresce para baixo, entao tem de sair
        // NEGATIVO -- inverter errado faz a pagina andar ao contrario,
        // que e o tipo de coisa que so se descobre usando.
        let saida = translate(&[rel(REL_WHEEL, 1)], true);
        assert_eq!(saida, vec![Event::Scroll { dx: 0.0, dy: -PASSO_RODA }]);
    }

    #[test]
    fn movimento_sai_antes_dos_cliques_do_lote() {
        let saida = translate(&[tecla(EV_BTN_LEFT, 1), rel(REL_X, 2)], true);
        assert_eq!(saida[0], Event::MouseRel { dx: 2.0, dy: 0.0 });
        assert_eq!(saida[1], Event::Button { code: BTN_LEFT, pressed: true });
    }

    #[test]
    fn lote_vazio_nao_produz_nada() {
        assert!(translate(&[], true).is_empty());
    }
}

// --- socorro ---------------------------------------------------------

/// `KEY_LEFTCTRL` e `KEY_RIGHTCTRL` do evdev.
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_RIGHTCTRL: u32 = 97;

/// Os dois Ctrl juntos: solta tudo e volta para a maquina local.
///
/// Precisa existir um caminho de volta que **nao dependa da rede**.
/// Agarrado, o teclado nao existe para o desktop local -- se o alvo
/// cair, sem isto a pessoa fica sem teclado na propria maquina e a
/// unica saida e o mouse (que tambem esta agarrado) ou outra maquina.
///
/// Os dois Ctrl porque nenhum programa usa a combinacao e nenhum texto a
/// produz: nao da para disparar sem querer digitando. E o mesmo gesto
/// que switch KVM de hardware usa, pelo mesmo motivo.
pub fn is_rescue(pressionadas: &std::collections::HashSet<u32>) -> bool {
    pressionadas.contains(&KEY_LEFTCTRL) && pressionadas.contains(&KEY_RIGHTCTRL)
}

#[cfg(test)]
mod testes_socorro {
    use super::*;
    use std::collections::HashSet;

    fn conj(v: &[u32]) -> HashSet<u32> {
        v.iter().copied().collect()
    }

    #[test]
    fn os_dois_ctrl_pedem_socorro() {
        assert!(is_rescue(&conj(&[KEY_LEFTCTRL, KEY_RIGHTCTRL])));
    }

    #[test]
    fn um_ctrl_so_nao_pede() {
        assert!(!is_rescue(&conj(&[KEY_LEFTCTRL])));
        assert!(!is_rescue(&conj(&[KEY_RIGHTCTRL])));
    }

    #[test]
    fn ctrl_c_nao_pede() {
        // 46 = KEY_C. Atalho comum nao pode soltar o KVM.
        assert!(!is_rescue(&conj(&[KEY_LEFTCTRL, 46])));
    }

    #[test]
    fn os_dois_ctrl_com_mais_teclas_ainda_pedem() {
        // A pessoa em panico aperta tudo. Exigir exclusividade faria o
        // socorro falhar exatamente quando ele e preciso.
        assert!(is_rescue(&conj(&[KEY_LEFTCTRL, KEY_RIGHTCTRL, 42, 56])));
    }
}
