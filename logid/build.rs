//! Carimba o build com o commit, para o deploy conferir sincronia.
//!
//! Vem de variavel de ambiente e nao de `git` direto porque o Mac compila
//! a partir de uma copia rsync SEM o `.git` -- chamar git la daria
//! "desconhecido" justamente na maquina que ficou para tras uma vez.
//! Quem sabe o commit e quem dispara o deploy; ele passa adiante.
fn main() {
    println!("cargo:rerun-if-env-changed=LOGI_BUILD");
    let v = std::env::var("LOGI_BUILD").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "desconhecido".into())
    });
    println!("cargo:rustc-env=LOGI_BUILD={v}");
}
