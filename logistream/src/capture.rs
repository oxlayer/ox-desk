//! De onde vem o quadro.
//!
//! Linux/Wayland: portal `ScreenCast` + PipeWire. Nao ha atalho -- o
//! compositor nao deixa ninguem ler a tela sem passar por ele, que e a
//! mesma razao de a borda falar com o `Mutter.InputCapture` em vez de ler
//! o cursor.
//!
//! Windows: DXGI Desktop Duplication.

/// Um quadro cru, como a plataforma entregou.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Nanossegundos do relogio monotonico. Sem isto nao ha como medir
    /// latencia fim-a-fim, e sem medir nao ha como melhorar.
    pub captured_at_ns: u64,
    /// BGRA linear, ou um handle de textura quando a captura for
    /// zero-copy. Comeca simples.
    pub data: Vec<u8>,
}

/// Fonte de quadros. Trait porque sao tres implementacoes e elas nao
/// compartilham nada alem desta forma.
pub trait Capture: Send {
    fn next_frame(&mut self) -> anyhow::Result<Frame>;
    fn size(&self) -> (u32, u32);
}
