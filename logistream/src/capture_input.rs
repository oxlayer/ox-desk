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
//! - **Os codigos ja sao os nossos.** `Evento::Tecla` carrega keycode de
//!   evdev porque e o que o `NotifyKeyboardKeycode` do Mutter recebe, e
//!   `scancode_de_evdev()` ja traduz para o Windows. Nao ha conversao
//!   nova no caminho.
//! - **Independe de compositor.** Mesmo codigo em qualquer Linux.
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

use crate::input::{Evento, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};

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

/// Evento cru do kernel, ja desempacotado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cru {
    pub tipo: u16,
    pub codigo: u16,
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
pub fn traduzir(lote: &[Cru]) -> Vec<Evento> {
    let mut saida = Vec::new();
    let (mut dx, mut dy) = (0.0f64, 0.0f64);
    let (mut rx, mut ry) = (0.0f64, 0.0f64);

    for e in lote {
        match e.tipo {
            EV_REL => match e.codigo {
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
                // valor 2 = autorepeat do kernel. Descartado: o outro
                // lado repete sozinho a partir do key-down, e reenviar
                // produziria repeticao dupla.
                if e.valor == 2 {
                    continue;
                }
                let apertado = e.valor == 1;
                match e.codigo {
                    EV_BTN_LEFT => saida.push(Evento::Botao { codigo: BTN_LEFT, apertado }),
                    EV_BTN_RIGHT => saida.push(Evento::Botao { codigo: BTN_RIGHT, apertado }),
                    EV_BTN_MIDDLE => saida.push(Evento::Botao { codigo: BTN_MIDDLE, apertado }),
                    cod => saida.push(Evento::Tecla { keycode: u32::from(cod), apertado }),
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
        saida.insert(0, Evento::MouseRel { dx, dy });
    }
    if rx != 0.0 || ry != 0.0 {
        saida.push(Evento::Rolagem { dx: rx, dy: ry });
    }
    saida
}

/// Um `/dev/input/eventN` aberto.
pub struct Dispositivo {
    arquivo: File,
    pub nome: String,
    pub caminho: PathBuf,
    agarrado: bool,
    lote: Vec<Cru>,
}

impl Dispositivo {
    pub fn abrir(caminho: &Path) -> std::io::Result<Self> {
        let arquivo = File::open(caminho)?;
        let nome = nome_de(caminho).unwrap_or_else(|| "?".into());
        Ok(Dispositivo { arquivo, nome, caminho: caminho.to_path_buf(), agarrado: false, lote: Vec::new() })
    }

    /// Todos os `/dev/input/event*` que conseguimos abrir.
    ///
    /// Os que recusam acesso sao PULADOS em silencio de proposito: a
    /// maquina tem dezenas de nos de input (botao de energia, webcam,
    /// alto-falante) e nenhum deles nos interessa. Quem reclama de
    /// permissao e `exigir()`, com o nome do dispositivo que faltou.
    pub fn listar() -> Vec<Dispositivo> {
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
            if let Ok(d) = Dispositivo::abrir(&c) {
                v.push(d);
            }
        }
        v
    }

    /// O dispositivo para de existir para o desktop local.
    pub fn agarrar(&mut self) -> std::io::Result<()> {
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

    pub fn soltar(&mut self) -> std::io::Result<()> {
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

    pub fn agarrado(&self) -> bool {
        self.agarrado
    }

    /// Le o que houver e devolve os eventos dos lotes COMPLETOS.
    ///
    /// Um lote so fecha no `SYN_REPORT`. Eventos de um lote ainda aberto
    /// ficam guardados para a proxima chamada -- entregar meio lote
    /// mandaria o eixo X sem o Y.
    pub fn ler(&mut self) -> std::io::Result<Vec<Evento>> {
        let mut buf = [0u8; TAM_EVENTO * 64];
        let n = self.arquivo.read(&mut buf)?;
        let mut saida = Vec::new();
        for pedaco in buf[..n].chunks_exact(TAM_EVENTO) {
            let tipo = u16::from_ne_bytes([pedaco[16], pedaco[17]]);
            let codigo = u16::from_ne_bytes([pedaco[18], pedaco[19]]);
            let valor = i32::from_ne_bytes([pedaco[20], pedaco[21], pedaco[22], pedaco[23]]);
            if tipo == EV_SYN {
                saida.extend(traduzir(&self.lote));
                self.lote.clear();
            } else {
                self.lote.push(Cru { tipo, codigo, valor });
            }
        }
        Ok(saida)
    }
}

impl Drop for Dispositivo {
    /// Soltar no `Drop` e cinto de seguranca, nao o mecanismo.
    ///
    /// O kernel ja solta o grab quando o fd fecha, entao teclado travado
    /// para sempre nao e um estado alcancavel nem se o processo for
    /// morto com SIGKILL. Isto so torna a ordem explicita para quem le.
    fn drop(&mut self) {
        let _ = self.soltar();
    }
}

/// Nome do dispositivo, pelo sysfs.
///
/// Pelo sysfs e nao pelo ioctl `EVIOCGNAME`: o arquivo do sysfs pode ser
/// lido sem abrir o no de input, entao da para listar e escolher ANTES
/// de ter permissao -- que e o que permite dizer "falta ACL no MX Keys"
/// em vez de "nenhum dispositivo encontrado".
pub fn nome_de(caminho: &Path) -> Option<String> {
    let base = caminho.file_name()?.to_str()?;
    let p = format!("/sys/class/input/{base}/device/name");
    Some(std::fs::read_to_string(p).ok()?.trim().to_string())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn rel(codigo: u16, valor: i32) -> Cru {
        Cru { tipo: EV_REL, codigo, valor }
    }
    fn tecla(codigo: u16, valor: i32) -> Cru {
        Cru { tipo: EV_KEY, codigo, valor }
    }

    #[test]
    fn eixos_do_mesmo_lote_viram_um_movimento_so() {
        let saida = traduzir(&[rel(REL_X, 5), rel(REL_Y, -3)]);
        assert_eq!(saida, vec![Evento::MouseRel { dx: 5.0, dy: -3.0 }]);
    }

    #[test]
    fn lote_sem_movimento_nao_manda_movimento() {
        let saida = traduzir(&[tecla(30, 1)]);
        assert_eq!(saida, vec![Evento::Tecla { keycode: 30, apertado: true }]);
    }

    #[test]
    fn autorepeat_do_kernel_e_descartado() {
        // valor 2 = repeticao. O outro lado repete sozinho a partir do
        // key-down; reenviar daria repeticao dupla.
        let saida = traduzir(&[tecla(30, 1), tecla(30, 2), tecla(30, 2), tecla(30, 0)]);
        assert_eq!(
            saida,
            vec![
                Evento::Tecla { keycode: 30, apertado: true },
                Evento::Tecla { keycode: 30, apertado: false },
            ]
        );
    }

    #[test]
    fn botoes_viram_botao_e_nao_tecla() {
        let saida = traduzir(&[tecla(EV_BTN_LEFT, 1), tecla(EV_BTN_RIGHT, 0)]);
        assert_eq!(
            saida,
            vec![
                Evento::Botao { codigo: BTN_LEFT, apertado: true },
                Evento::Botao { codigo: BTN_RIGHT, apertado: false },
            ]
        );
    }

    #[test]
    fn roda_para_cima_rola_para_cima() {
        // No evdev, roda para cima e +1. No nosso protocolo (e no
        // Wayland) o eixo Y cresce para baixo, entao tem de sair
        // NEGATIVO -- inverter errado faz a pagina andar ao contrario,
        // que e o tipo de coisa que so se descobre usando.
        let saida = traduzir(&[rel(REL_WHEEL, 1)]);
        assert_eq!(saida, vec![Evento::Rolagem { dx: 0.0, dy: -PASSO_RODA }]);
    }

    #[test]
    fn movimento_sai_antes_dos_cliques_do_lote() {
        let saida = traduzir(&[tecla(EV_BTN_LEFT, 1), rel(REL_X, 2)]);
        assert_eq!(saida[0], Evento::MouseRel { dx: 2.0, dy: 0.0 });
        assert_eq!(saida[1], Evento::Botao { codigo: BTN_LEFT, apertado: true });
    }

    #[test]
    fn lote_vazio_nao_produz_nada() {
        assert!(traduzir(&[]).is_empty());
    }
}
