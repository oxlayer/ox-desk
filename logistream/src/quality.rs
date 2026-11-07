//! Quantos bits por segundo o encoder pode gastar.
//!
//! Spec: ../../specs/012-outside-the-lan.md
//!
//! ## Por que virou módulo
//!
//! A fórmula estava escrita **duas vezes**: em `nvenc.rs` (Linux e
//! Windows) e de novo, à mão, em `host_mac.rs`. Duas cópias da mesma
//! conta é como o alvo errado de 8 Mbps sobreviveu semanas -- corrigir
//! uma e esquecer a outra não dá erro, dá divergência silenciosa entre
//! máquinas.
//!
//! ## Por que um teto, e por que ele importa agora
//!
//! O alvo escala com a área da tela: ~8 Mbps em 1080p, ~15 em 1440p,
//! ~28 na Retina do MacBook. Dentro de casa isso é banda sobrando na
//! mesa e a imagem fica boa.
//!
//! Fora de casa o gargalo deixa de ser a LAN e passa a ser o **upload**
//! da casa, que em link residencial é uma fração do download. Mandar 15
//! Mbps por um upload de 10 enche a fila do roteador: a latência sobe,
//! os pacotes caem, e a tela desmonta -- que é pior do que uma imagem
//! mais simples entregue na hora.
//!
//! ## O que isto NÃO é
//!
//! Não é controle de congestionamento. O alvo é escolhido uma vez e não
//! se mexe sozinho; se o link piorar no meio da sessão, nada reage. O
//! `quinn` já mede RTT e perda do caminho (`Connection::stats()`), e
//! realimentar o encoder com isso é o passo seguinte -- o BUD da Parsec
//! é basicamente esse laço. Um teto escolhido à mão resolve "15 Mbps
//! não cabem no meu upload"; só a adaptação resolve "o Wi-Fi do hotel
//! varia".

use std::sync::OnceLock;

/// Piso e teto absolutos da escala por área.
pub const PISO: u64 = 8_000_000;
pub const TETO: u64 = 60_000_000;

static LIMITE: OnceLock<u64> = OnceLock::new();

/// Fixa um teto em bits/s. Chamar na partida, antes de criar encoder.
pub fn set_cap(bits: u64) {
    let _ = LIMITE.set(bits);
}

pub fn limite() -> Option<u64> {
    LIMITE.get().copied()
}

/// Bits por segundo para uma tela de `w`x`h`, respeitando o teto.
///
/// 4 bits por pixel por segundo é o que produz 8 / 15 / 33 Mbps em
/// 1080p / 1440p / 4K, e foi medido como a faixa em que a imagem para
/// de borrar ao rolar a página.
pub fn alvo_bits(w: u32, h: u32, teto: Option<u64>) -> u32 {
    let por_area = ((w as u64) * (h as u64) * 4).clamp(PISO, TETO);
    // O teto do usuário vence o PISO de propósito.
    //
    // Se ele disse "meu upload tem 5 Mbps", entregar 8 porque "é o
    // mínimo para ficar bonito" é entregar uma imagem que não chega.
    // Piso existe para a LAN, onde o custo de errar para baixo é só
    // quality; aqui o custo de errar para cima é a sessão inteira.
    match teto {
        Some(t) if t > 0 => por_area.min(t) as u32,
        _ => por_area as u32,
    }
}

/// O alvo desta máquina, já com o teto do processo aplicado.
pub fn target(w: u32, h: u32) -> u32 {
    alvo_bits(w, h, limite())
}

/// Lê `--mbps N` da linha de comando. `None` = sem teto.
pub fn cap_from_args(a: &[String]) -> Result<Option<u64>, String> {
    let Some(i) = a.iter().position(|v| v == "--mbps") else { return Ok(None) };
    let Some(v) = a.get(i + 1) else {
        return Err("--mbps precisa de um número (ex: --mbps 6)".into());
    };
    let n: f64 = v.parse().map_err(|_| format!("--mbps: '{v}' não é número"))?;
    if !(0.1..=1000.0).contains(&n) {
        return Err(format!("--mbps: {n} fora de 0.1..1000"));
    }
    Ok(Some((n * 1_000_000.0) as u64))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn escala_com_a_area() {
        // Os três números que o comentário sempre prometeu, e que a
        // fórmula antiga (`pixels * 60 / 1000`) não entregava: ela caía
        // no piso de 8 Mbps em qualquer resolução.
        assert_eq!(alvo_bits(1920, 1080, None), 8_294_400);
        assert_eq!(alvo_bits(2560, 1440, None), 14_745_600);
        assert_eq!(alvo_bits(3840, 2160, None), 33_177_600);
    }

    #[test]
    fn retina_do_mac() {
        // 3360x2100 -- a tela que denunciou o bug antigo ao anunciar
        // "8 Mbps" como as outras.
        assert_eq!(alvo_bits(3360, 2100, None), 28_224_000);
    }

    #[test]
    fn teto_corta_abaixo_do_piso() {
        // A regra que mais importa fora de casa: o teto do usuário
        // vence o piso. 5 Mbps pedidos têm de sair 5, não 8.
        assert_eq!(alvo_bits(2560, 1440, Some(5_000_000)), 5_000_000);
        assert_eq!(alvo_bits(1920, 1080, Some(5_000_000)), 5_000_000);
    }

    #[test]
    fn teto_folgado_nao_muda_nada() {
        assert_eq!(alvo_bits(1920, 1080, Some(50_000_000)), 8_294_400);
    }

    #[test]
    fn tela_minuscula_respeita_o_piso_sem_teto() {
        assert_eq!(alvo_bits(640, 480, None), PISO as u32);
    }

    #[test]
    fn tela_gigante_respeita_o_teto_absoluto() {
        assert_eq!(alvo_bits(7680, 4320, None), TETO as u32);
    }

    #[test]
    fn args_parseia_e_recusa() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(cap_from_args(&a(&["9777"])).unwrap(), None);
        assert_eq!(cap_from_args(&a(&["--mbps", "6"])).unwrap(), Some(6_000_000));
        assert_eq!(cap_from_args(&a(&["--mbps", "2.5"])).unwrap(), Some(2_500_000));
        assert!(cap_from_args(&a(&["--mbps"])).is_err());
        assert!(cap_from_args(&a(&["--mbps", "muito"])).is_err());
        assert!(cap_from_args(&a(&["--mbps", "0"])).is_err());
    }
}
