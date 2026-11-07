//! Stamps the build with the commit, so the deploy can check sync.
//!
//! It comes from an environment variable and not from `git` directly
//! because the Mac builds from an rsync copy WITHOUT `.git` -- calling git
//! there would give "desconhecido" precisely on the machine that once fell
//! behind. Whoever triggers the deploy knows the commit; it passes it on.
fn main() {
    println!("cargo:rerun-if-env-changed=LOGI_BUILD");
    println!("cargo:rerun-if-env-changed=LOGI_BUILD_N");
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

    // An incremental number besides the hash: the hash says two machines
    // are DIFFERENT, not which one is behind. `rev-list --count` always
    // grows and is the same on all three for the same commit.

    let n = std::env::var("LOGI_BUILD_N").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "0".into())
    });
    println!("cargo:rustc-env=LOGI_BUILD_N={n}");
}
