//! O que Linux e macOS fazem igual.
//!
//! Existe para nao haver uma terceira copia de hora local, diretorio de
//! config e construtor de filho. A diferenca entre os dois SOs esta em
//! outro lugar -- journal contra arquivo de log, systemd contra launchd,
//! barreira do mutter contra nada -- e e la que os modulos divergem.

use std::path::PathBuf;
use std::process::Command;

/// Hora local HH:MM:SS, sem puxar uma crate de data so para isso.
pub fn hhmmss() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let t = (secs + offset_secs()).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

fn offset_secs() -> i64 {
    let Ok(out) = Command::new("date").arg("+%z").output() else { return 0 };
    let z = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if z.len() < 5 {
        return 0;
    }
    let sign = if z.starts_with('-') { -1 } else { 1 };
    let h: i64 = z[1..3].parse().unwrap_or(0);
    let m: i64 = z[3..5].parse().unwrap_or(0);
    sign * (h * 3600 + m * 60)
}

/// `$XDG_CONFIG_HOME/logi` ou `~/.config/logi`, nos dois.
///
/// No macOS o lugar nativo seria `~/Library/Application Support`. Fica
/// `~/.config` de proposito: esta config e editada a mao, e um caminho
/// igual nas duas maquinas Unix e uma divergencia menos entre o daemon e
/// a GUI, que tambem precisa acerta-lo.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".config")
    });
    base.join("logi")
}

/// Ponto de partida do navegador de arquivos.
pub fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| "/".into())
}

/// Constroi um `Command` para um filho do daemon.
///
/// Aqui nao ha nada a ajustar; existe para o codigo comum chamar o mesmo
/// construtor nas tres plataformas. Ver a versao Windows, onde a flag
/// `CREATE_NO_WINDOW` evita janela de console piscando.
pub fn child(prog: &str) -> Command {
    Command::new(prog)
}
