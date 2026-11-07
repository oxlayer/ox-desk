//! Tela remota propria. Spec: ../specs/010-remote-desktop.md
//!
//! As fronteiras importam mais que a primeira implementacao: o primeiro
//! encoder que funcionar nao sera o ultimo, e trocar NVENC por
//! VideoToolbox ou x264 nao pode significar reescrever o pipeline.
//!
//! ## Como os pedacos se dividem
//!
//! O que e IGUAL nas plataformas mora em `serve` (laco de video, fatias,
//! saida por ociosidade) e `nvenc` (codificacao e qualidade). O que
//! genuinamente difere mora em par: `capture`/`capture_win` para a
//! origem do quadro, e a injecao de input (`host_linux` fala com o
//! Mutter por D-Bus, `input_win` chama `SendInput`).
//!
//! Os modulos de plataforma sao `#![cfg(...)]` por dentro, entao no SO
//! errado compilam VAZIOS em vez de dar erro -- o Mac compila esta
//! biblioteca inteira para gerar so o visualizador.

pub mod bind;
pub mod capture;
pub mod capture_input;
pub mod capture_win;
pub mod cursor;
pub mod decode;
pub mod decode_nv;
pub mod encode;
pub mod host_linux;
pub mod host_mac;
pub mod host_win;
pub mod identity;
pub mod input;
pub mod input_mac;
pub mod input_win;
pub mod net;
pub mod nvenc;
pub mod qualidade;
pub mod serve;
