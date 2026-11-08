//! Stamps the build with the commit, so the deploy can check sync.
//!
//! It comes from an environment variable and not from `git` directly
//! because the Mac builds from an rsync copy WITHOUT `.git` -- calling git
//! there would give "unknown" precisely on the machine that once fell
//! behind. Whoever triggers the deploy knows the commit; it passes it on.
fn main() {
    println!("cargo:rerun-if-env-changed=OXDESK_BUILD");
    println!("cargo:rerun-if-env-changed=OXDESK_BUILD_N");
    // Raising the base has to force a rebuild, or the binary keeps
    // announcing the old number and the updater keeps deciding on it.
    println!("cargo:rerun-if-changed=../BUILD_BASE");
    let v = std::env::var("OXDESK_BUILD").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".into())
    });
    println!("cargo:rustc-env=OXDESK_BUILD={v}");

    // An incremental number besides the hash: the hash says two machines
    // are DIFFERENT, not which one is behind. `rev-list --count` always
    // grows and is the same on all three for the same commit.

    // BUILD_BASE + the count, and the base matters MORE here than in the
    // deploy, because this is the path a plain `cargo build` takes.
    //
    // The squash of 2026-11-08 reset `rev-list --count` to 1. A build that
    // reports a number below the newest published package's makes the
    // updater decide `Update` and install OLDER code over the machine --
    // `decide()` in ota/src/release.rs only refuses when `n_there <
    // n_here`. I fixed deploy.sh, build-binaries.sh and the workflow and
    // left this one out, and the next local build announced itself as #6
    // against #229 published.
    let n = std::env::var("OXDESK_BUILD_N").unwrap_or_else(|_| {
        let base: u64 = std::fs::read_to_string("../BUILD_BASE")
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0);
        let counted: u64 = std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(0);
        (base + counted).to_string()
    });
    println!("cargo:rustc-env=OXDESK_BUILD_N={n}");
}
