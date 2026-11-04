//! Onde o daemon fala.
//!
//! No Linux o journal timbra e guarda, e um `println!` basta. No Windows
//! nao existe journal: se o daemon so escrevesse em stdout, a GUI de la
//! nao teria log nenhum -- e foi justamente a falta de log que, do lado
//! Linux, me fez diagnosticar errado uma troca de 105 s.
//!
//! Dai estes macros: uma linha, dois destinos. O stdout continua para
//! quem roda o daemon na mao, e `sys::log_sink` persiste onde a
//! plataforma precisar (no Linux, nada -- o journal ja fez).

#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        $crate::sys::log_sink(&line);
    }};
}

#[macro_export]
macro_rules! gripe {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        eprintln!("{line}");
        $crate::sys::log_sink(&line);
    }};
}
