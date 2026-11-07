//! Quadro cru -> H.264.
//!
//! NVENC **direto**, via `libnvidia-encode`, e nao pelo libavcodec: o
//! ffmpeg do linuxbrew veio sem o encoder nvenc (`ffmpeg -encoders` lista
//! so `libx264` e `h264_v4l2m2m`), e de todo jeito o libavcodec seria uma
//! camada a mais no caminho quente. A biblioteca ja esta na maquina --
//! vem com o driver (`libnvidia-encode.so.1`, 580.178.04).
//!
//! Os dois hosts sao NVIDIA (RTX 3060 e RTX 3050), entao e um caminho de
//! codificacao, nao tres.

/// Um pacote codificado, pronto para empacotar e mandar.
pub struct Packet {
    pub data: Vec<u8>,
    /// Quadro-chave: o cliente que entra no meio precisa de um para
    /// comecar a draw.
    pub keyframe: bool,
    pub captured_at_ns: u64,
}

pub trait Encoder: Send {
    fn encode(&mut self, frame: &crate::capture::Frame) -> anyhow::Result<Vec<Packet>>;
    /// Pedido de quadro-chave -- o cliente pede quando perde contexto.
    fn request_keyframe(&mut self);
}
