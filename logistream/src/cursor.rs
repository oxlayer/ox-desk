//! Composicao do cursor no quadro capturado.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! Fica num modulo proprio, sem `cfg(windows)`, por um motivo
//! pratico: a logica e aritmetica de pixel e nao depende de plataforma
//! nenhuma, e enquanto ela morava em `capture_win.rs` os testes dela
//! **nunca rodavam** -- aquele arquivo inteiro e compilado so no
//! Windows, e a suite roda no Linux. Teste que nao executa e
//! documentacao com aparencia de garantia.
//!
//! Mesma decisao que levou `scancode_from_evdev` para `input.rs`.

//// O DXGI entrega a tela **sem** o ponteiro: a posicao vem no
// `DXGI_OUTDUPL_FRAME_INFO` e o desenho em `GetFramePointerShape`,
// porque e assim que o hardware compoe.
//
// Isso nao incomodava com o visualizador: ele manda posicao ABSOLUTA,
// entao o cursor da propria maquina de quem assiste fica exatamente
// sobre o ponto remoto -- com zero latencia, inclusive. Com o KVM o
// truque desaba: o ponteiro local esta agarrado e parado, e o remoto
// nunca foi desenhado. Nao sobra cursor nenhum, e a pessoa ve os
// efeitos de hover sem saber onde esta.

/// Formato do desenho do ponteiro, como o DXGI classifica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    /// 1 bpp, mascara AND seguida de mascara XOR; altura e o dobro.
    Mono,
    /// 32 bpp BGRA com alfa de verdade.
    Color,
    /// 32 bpp onde o alfa e mascara: 0 = pinta, 0xFF = inverte o fundo.
    MaskedColor,
}

/// Um desenho de ponteiro, como veio do DXGI.
#[derive(Debug, Clone)]
pub struct Shape {
    pub tipo: ShapeKind,
    pub w: u32,
    pub h: u32,
    pub pitch: usize,
    pub bytes: Vec<u8>,
    /// Ponto do desenho que fica sob a coordenada do ponteiro.
    pub hot: (i32, i32),
}

/// Retangulo coberto por uma forma desenhada em (x, y), recortado na tela.
///
/// Devolve `None` quando nao sobra nada visivel -- cursor na borda sai
/// parcialmente, e cursor fora da tela nao desenha. Separado do desenho
/// porque e exatamente o mesmo calculo que o "salvar o que estava
/// embaixo" precisa, e as duas contas divergirem deixaria rastro de
/// cursor na tela.
pub fn clip(
    tela_w: u32,
    tela_h: u32,
    f_w: u32,
    f_h: u32,
    x: i32,
    y: i32,
) -> Option<(u32, u32, u32, u32)> {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + f_w as i32).min(tela_w as i32);
    let y1 = (y + f_h as i32).min(tela_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Desenha a forma em (x, y) sobre pixels BGRA.
///
/// `x`/`y` ja descontam o hotspot. Funcao pura sobre o buffer: e aqui
/// que mora a decisao de cada formato, e e isto que os testes olham.
pub fn draw(px: &mut [u8], stride: usize, tela_w: u32, tela_h: u32, f: &Shape, x: i32, y: i32) {
    // Mono guarda AND e XOR empilhadas, entao a altura util e metade.
    let alt = if f.tipo == ShapeKind::Mono { f.h / 2 } else { f.h };
    let Some((rx, ry, rw, rh)) = clip(tela_w, tela_h, f.w, alt, x, y) else {
        return;
    };

    for ly in 0..rh {
        let fy = (ry + ly) as i32 - y;
        for lx in 0..rw {
            let fx = (rx + lx) as i32 - x;
            let destino = (ry + ly) as usize * stride + (rx + lx) as usize * 4;
            if destino + 3 >= px.len() {
                continue;
            }

            match f.tipo {
                ShapeKind::Mono => {
                    // Um bit por pixel, MSB primeiro. `and` 0 pinta a
                    // cor; `and` 1 com `xor` 1 inverte o que estava
                    // embaixo -- e assim que o cursor de texto fica
                    // visivel sobre fundo claro e escuro.
                    let byte = fy as usize * f.pitch + (fx as usize) / 8;
                    let bit = 7 - (fx % 8);
                    let meio = (alt as usize) * f.pitch;
                    let Some(&a) = f.bytes.get(byte) else { continue };
                    let Some(&o) = f.bytes.get(meio + byte) else { continue };
                    let a = (a >> bit) & 1;
                    let o = (o >> bit) & 1;
                    for c in 0..3 {
                        let fundo = px[destino + c];
                        px[destino + c] = match (a, o) {
                            (0, 0) => 0x00,
                            (0, 1) => 0xFF,
                            (1, 0) => fundo,
                            _ => !fundo,
                        };
                    }
                }
                ShapeKind::Color => {
                    let o = fy as usize * f.pitch + fx as usize * 4;
                    if o + 3 >= f.bytes.len() {
                        continue;
                    }
                    let a = f.bytes[o + 3] as u32;
                    if a == 0 {
                        continue;
                    }
                    // Alfa normal. Sem gama de proposito: o cursor tem
                    // alfa quase binario nas bordas e a diferenca nao
                    // paga o custo por pixel.
                    for c in 0..3 {
                        let frente = f.bytes[o + c] as u32;
                        let fundo = px[destino + c] as u32;
                        px[destino + c] = ((frente * a + fundo * (255 - a)) / 255) as u8;
                    }
                }
                ShapeKind::MaskedColor => {
                    let o = fy as usize * f.pitch + fx as usize * 4;
                    if o + 3 >= f.bytes.len() {
                        continue;
                    }
                    // Aqui o canal "alfa" nao e transparencia: 0 manda
                    // copiar, 0xFF manda inverter o fundo.
                    let mascara = f.bytes[o + 3];
                    for c in 0..3 {
                        px[destino + c] = if mascara == 0 {
                            f.bytes[o + c]
                        } else {
                            px[destino + c] ^ f.bytes[o + c]
                        };
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod testes_cursor {
    use super::*;

    fn tela(w: usize, h: usize) -> Vec<u8> {
        vec![0u8; w * h * 4]
    }

    #[test]
    fn fora_da_tela_nao_desenha() {
        assert_eq!(clip(100, 100, 32, 32, -40, 10), None);
        assert_eq!(clip(100, 100, 32, 32, 10, 200), None);
    }

    #[test]
    fn na_borda_desenha_so_o_pedaco_visivel() {
        // Cursor com metade para fora da direita: sobram 10 colunas.
        assert_eq!(clip(100, 100, 32, 32, 90, 50), Some((90, 50, 10, 32)));
        // E para a esquerda, a origem anda junto.
        assert_eq!(clip(100, 100, 32, 32, -10, 50), Some((0, 50, 22, 32)));
    }

    #[test]
    fn cor_com_alfa_cheio_substitui_o_fundo() {
        let mut px = tela(4, 4);
        let f = Shape {
            tipo: ShapeKind::Color,
            w: 1,
            h: 1,
            pitch: 4,
            bytes: vec![10, 20, 30, 255],
            hot: (0, 0),
        };
        draw(&mut px, 16, 4, 4, &f, 1, 1);
        let o = 1 * 16 + 1 * 4;
        assert_eq!(&px[o..o + 3], &[10, 20, 30]);
    }

    #[test]
    fn cor_com_alfa_zero_nao_toca_no_fundo() {
        let mut px = tela(4, 4);
        px[0] = 99;
        let f = Shape {
            tipo: ShapeKind::Color,
            w: 1,
            h: 1,
            pitch: 4,
            bytes: vec![10, 20, 30, 0],
            hot: (0, 0),
        };
        draw(&mut px, 16, 4, 4, &f, 0, 0);
        assert_eq!(px[0], 99, "alfa zero tem de deixar o fundo intacto");
    }

    #[test]
    fn mono_pinta_preto_branco_e_inverte() {
        // Uma linha de 8 pixels. AND=0b0011_1111 XOR=0b0101_1111:
        //   pixel 0: and=0 xor=0 -> preto
        //   pixel 1: and=0 xor=1 -> branco
        //   pixel 2: and=1 xor=0 -> mantem o fundo
        //   pixel 3: and=1 xor=1 -> inverte o fundo
        let mut px = tela(8, 2);
        for i in 0..8 {
            px[i * 4] = 100;
            px[i * 4 + 1] = 100;
            px[i * 4 + 2] = 100;
        }
        let f = Shape {
            tipo: ShapeKind::Mono,
            w: 8,
            h: 2, // AND + XOR empilhadas: altura util = 1
            pitch: 1,
            bytes: vec![0b0011_1111, 0b0101_1111],
            hot: (0, 0),
        };
        draw(&mut px, 32, 8, 2, &f, 0, 0);
        assert_eq!(px[0], 0x00, "and=0 xor=0 -> preto");
        assert_eq!(px[4], 0xFF, "and=0 xor=1 -> branco");
        assert_eq!(px[8], 100, "and=1 xor=0 -> fundo intacto");
        assert_eq!(px[12], 155, "and=1 xor=1 -> fundo invertido");
    }

    #[test]
    fn mascarada_copia_ou_inverte() {
        let mut px = tela(2, 1);
        px[0] = 100;
        px[4] = 100;
        let f = Shape {
            tipo: ShapeKind::MaskedColor,
            w: 2,
            h: 1,
            pitch: 8,
            bytes: vec![7, 7, 7, 0x00, 0xFF, 0, 0, 0xFF],
            hot: (0, 0),
        };
        draw(&mut px, 8, 2, 1, &f, 0, 0);
        assert_eq!(px[0], 7, "mascara 0 copia a cor");
        assert_eq!(px[4], 100 ^ 0xFF, "mascara 0xFF inverte o fundo");
    }
}

// --- cursor proprio --------------------------------------------------
//
// O Windows **esconde o cursor quando a maquina nao tem mouse**, e no
// KVM ela nunca tem: o dongle fica do outro lado. Medido com
// `GetCursorInfo`: `flags=0` (escondido) e `hCursor=0` (nao existe
// cursor), com a posicao correta -- por isso o hover respondia e nada
// aparecia.
//
// Nessa situacao nao adianta pedir o desenho ao DXGI: ele reporta
// `Visible=false` e nunca manda forma nenhuma, porque de fato nao ha
// nada sendo desenhado. Quem precisa draw somos nos.

/// Seta padrao, 12x19, no formato de mascaras do cursor monocromatico.
///
/// Desenhada a mao em vez de pedida ao sistema (`LoadCursorW` +
/// `GetIconInfo` + `GetDIBits`) porque aqui ela e um FALLBACK: so entra
/// quando o sistema nao tem cursor para dar. Pedir a quem nao tem seria
/// circular, e o caminho de Win32 para extrair o bitmap tem mais
/// chances de falhar do que estas dezenove linhas.
///
/// `K` preto, `W` branco, `.` transparente. O contorno branco existe
/// para a seta nao sumir sobre fundo escuro -- e o mesmo motivo pelo
/// qual o cursor do sistema tem um.
const SETA: [&str; 19] = [
    "W...........",
    "WW..........",
    "WKW.........",
    "WKKW........",
    "WKKKW.......",
    "WKKKKW......",
    "WKKKKKW.....",
    "WKKKKKKW....",
    "WKKKKKKKW...",
    "WKKKKKKKKW..",
    "WKKKKKKKKKW.",
    "WKKKKKKWWWWW",
    "WKKKWKKW....",
    "WKKWWKKW....",
    "WKW..WKKW...",
    "WW...WKKW...",
    "W.....WKKW..",
    "......WKKW..",
    ".......WWW..",
];

/// Monta a seta embutida como uma `Shape` monocromatica.
///
/// Reaproveita o caminho `Mono` do `draw`, que ja trata AND/XOR --
/// um segundo caminho de desenho so para este caso seria mais code
/// para a mesma figura.
pub fn builtin_arrow() -> Shape {
    let w = SETA[0].len() as u32;
    let h = SETA.len() as u32;
    let pitch = w.div_ceil(8) as usize;
    // AND e XOR empilhadas, nesta ordem, como o DXGI entrega.
    let mut bytes = vec![0u8; pitch * h as usize * 2];
    let meio = pitch * h as usize;

    for (y, linha) in SETA.iter().enumerate() {
        for (x, c) in linha.bytes().enumerate() {
            let byte = y * pitch + x / 8;
            let bit = 7 - (x % 8);
            match c {
                // transparente: and=1, xor=0 -> mantem o fundo
                b'.' => bytes[byte] |= 1 << bit,
                // preto: and=0, xor=0
                b'K' => {}
                // branco: and=0, xor=1
                _ => bytes[meio + byte] |= 1 << bit,
            }
        }
    }

    Shape { tipo: ShapeKind::Mono, w, h: h * 2, pitch, bytes, hot: (0, 0) }
}

#[cfg(test)]
mod testes_seta {
    use super::*;

    #[test]
    fn a_ponta_da_seta_e_branca_e_o_miolo_preto() {
        let f = builtin_arrow();
        // Fundo cinza para distinguir "pintou" de "deixou passar".
        let mut px = vec![100u8; 32 * 32 * 4];
        draw(&mut px, 32 * 4, 32, 32, &f, 0, 0);

        let em = |x: usize, y: usize| px[y * 32 * 4 + x * 4];
        assert_eq!(em(0, 0), 0xFF, "a ponta e o contorno branco");
        assert_eq!(em(1, 2), 0x00, "o miolo e preto");
        assert_eq!(em(11, 0), 100, "fora da seta o fundo fica intacto");
    }

    #[test]
    fn a_seta_cabe_no_proprio_buffer() {
        let f = builtin_arrow();
        let alt = f.h / 2;
        assert_eq!(f.bytes.len(), f.pitch * alt as usize * 2);
        assert_eq!(alt, 19);
        assert_eq!(f.w, 12);
    }

    /// Desenhar na borda nao pode estourar o buffer.
    #[test]
    fn na_borda_nao_escreve_fora() {
        let f = builtin_arrow();
        let mut px = vec![0u8; 16 * 16 * 4];
        // Quatro cantos, todos com a seta parcialmente para fora.
        for (x, y) in [(-5, -5), (14, 14), (-5, 14), (14, -5)] {
            draw(&mut px, 16 * 4, 16, 16, &f, x, y);
        }
    }
}
