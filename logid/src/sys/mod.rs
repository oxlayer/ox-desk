//! O que difere entre Linux e Windows, e so isso.
//!
//! O `logid` nasceu so no Linux e a GUI com ele. O Windows ficou com um
//! CLI de um disparo (`logid-agent`), sem API, sem bandeja, sem janela --
//! e a GUI do Ubuntu alcancava a outra maquina por SSH, chamando o CLI na
//! mao. Resultado: a troca era simetrica (o `watch-edge` deu a borda ao
//! Windows) mas o control plane nao, e quem estava no Windows nao tinha
//! painel nenhum.
//!
//! Medido antes de decidir: dos nove modulos do daemon, seis sao
//! portaveis tal como estao. O que amarrava ao Linux eram cinco pontos --
//! `getent` no fallback de DNS, `systemctl` no estado dos units, `date
//! +%z` no fuso, `journalctl` na origem dos logs e o D-Bus do mutter na
//! borda. Dai este modulo: um lugar para os cinco, em vez de um segundo
//! daemon no `agent/` duplicando os outros seis.
//!
//! Contrato que cada plataforma cumpre:
//!
//! | funcao | Linux | macOS | Windows |
//! |---|---|---|---|
//! | `resolve_fallback` | `getent hosts` | nada (mDNS nativo) | nada (mDNS nativo) |
//! | `unit_state` | `systemctl --user` | `launchctl`, senao processo | servico, senao processo |
//! | `hhmmss` | `date +%z` (comum) | idem | `GetLocalTime` |
//! | `log_lines` | `journalctl --user -u` | arquivo em `~/Library/Logs` | arquivo em `%LOCALAPPDATA%` |
//! | `log_sink` | nada (o journal timbra) | anexa com hora | anexa com hora |
//! | `device_access` | ACL em `/dev/hidraw` | abrir receptor | abrir receptor |
//! | `spawn_border` | barreira do mutter | **nao ha** (ver macos.rs) | `GetCursorPos` |
//!
//! O que Linux e macOS fazem igual vive em `unix.rs`, para nao existir
//! uma terceira copia de hora local e diretorio de config.

#[cfg(unix)]
mod unix;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// O que um cliente de tela sabe fazer, e como pedir.
///
/// Existe porque a UI oferece as opcoes no estilo Parsec -- teclado e
/// mouse, som, microfone -- e oferecer um check que nao faz nada e pior
/// que nao oferecer. Cada cliente declara o que honra; o que ele nao
/// honra aparece desabilitado com o motivo.
///
/// O que de fato muda por cliente, medido na documentacao de cada um:
///
/// | cliente | controle | som | microfone |
/// |---|---|---|---|
/// | `xfreerdp3` | sempre | `/sound` | `/microphone` |
/// | `gnome-connections` | sempre | nao expoe | nao expoe |
/// | `mstsc` | sempre | so por arquivo .rdp | idem |
/// | `vncviewer` | `-ViewOnly` desliga | protocolo nao tem | nao tem |
/// | `open vnc://` (macOS) | sempre | protocolo nao tem | nao tem |
pub struct ClientSpec {
    pub prog: &'static str,
    pub args: &'static [&'static str],
    /// Argumentos extras quando o som e pedido. `None` = nao sabe fazer.
    pub audio: Option<&'static [&'static str]>,
    pub mic: Option<&'static [&'static str]>,
    /// Argumentos para entrar SEM teclado e mouse. `None` = o cliente
    /// sempre controla.
    pub view_only: Option<&'static [&'static str]>,
}
