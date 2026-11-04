//! Tela remota propria. Spec: ../specs/010-remote-desktop.md
//!
//! Ainda nao faz nada util -- e o esqueleto com as fronteiras ja postas.
//! As fronteiras importam mais que a primeira implementacao: o primeiro
//! encoder que funcionar nao sera o ultimo, e trocar NVENC por
//! VideoToolbox ou x264 nao pode significar reescrever o pipeline.

pub mod capture;
pub mod encode;
