//! What the GitHub Release says, and what to do about it.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Everything here is a pure function over text: the JSON the API
//! returned, the `SHA256SUMS` that came with the Release, the package
//! name. The network, the disk and each system's commands live in
//! `apply.rs` -- the decision "is this newer and worth swapping?" has to
//! be testable without any GitHub around.
//!
//! Error strings end up in the state's message, which the panel shows:
//! they stay in Portuguese.

use crate::plan::System;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// The API address (`.../releases/assets/<id>`), not the
    /// `browser_download_url`.
    ///
    /// On a private repo the `browser_download_url` answers **404** even
    /// with a token: it serves the CDN, which does not know about the
    /// authentication. The API address serves the file when asked with
    /// `Accept: application/octet-stream`.
    pub url: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<Asset>,
}

/// Parses the `/releases/latest` response.
pub fn parse(json: &str) -> Result<Release, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("the answer is not JSON: {e}"))?;

    // The API answers 200 with `{"message": "..."}` in several cases
    // (no token on a private repo, rate limit exceeded). Without this the
    // error would show up as "no package for linux", which sends you
    // looking in the wrong place.
    if v.get("assets").is_none() {
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("no assets and no message");
        return Err(format!("GitHub answered: {msg}"));
    }

    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("?").to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x.get("name")?.as_str()?.to_string(),
                        url: x.get("url")?.as_str()?.to_string(),
                        bytes: x.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Release { tag, assets })
}

/// This system's package, by the prefix `build-binaries.sh` uses.
pub fn pick<'a>(l: &'a Release, s: System) -> Result<&'a Asset, String> {
    let p = crate::plan::prefix(s);
    l.assets
        .iter()
        .find(|a| a.name.starts_with(p) && a.name.ends_with(".tar.gz"))
        .ok_or_else(|| {
            let has: Vec<&str> = l.assets.iter().map(|a| a.name.as_str()).collect();
            format!("no package starting with {p}; the Release has: {}", has.join(", "))
        })
}

/// The Release's `SHA256SUMS`.
pub fn sums<'a>(l: &'a Release) -> Result<&'a Asset, String> {
    l.assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or_else(|| "the Release has no SHA256SUMS; I cannot check what I downloaded".to_string())
}

/// The `-<n>-<hash>` the package name carries.
///
/// `oxdesk-linux-x86_64-124-abc1234.tar.gz` -> `(124, "abc1234")`. The rule
/// is "the last two fields before `.tar.gz`", which also holds for
/// `oxdesk-windows-x86_64-unsigned-...`, which has one extra field in the
/// middle.
pub fn stamp_from_name(name: &str) -> Option<(u64, String)> {
    let base = name.strip_suffix(".tar.gz")?;
    let mut fields = base.rsplit('-');
    let hash = fields.next()?;
    let n: u64 = fields.next()?.parse().ok()?;
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((n, hash.to_string()))
}

/// The expected sum of a file, read from a `SHA256SUMS`.
///
/// `sha256sum`/`shasum` format: `<64 hex><two spaces><name>`. The name
/// may come with a path (`dist/x.tar.gz`) because whoever generated it
/// may have run from another directory; the base name is compared.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    lines(sums).into_iter().find(|(_, n)| n == name).map(|(s, _)| s)
}

/// Every line of a `SHA256SUMS`, as `(sum, base name)`.
pub fn lines(sums: &str) -> Vec<(String, String)> {
    sums.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (sum, rest) = l.split_once(' ')?;
            if sum.len() != 64 || !sum.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            // The binary-mode `*` and the path separator are dropped.
            let name = rest.trim().trim_start_matches('*');
            let name = name.rsplit(['/', '\\']).next()?;
            Some((sum.to_ascii_lowercase(), name.to_string()))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// What is in the Release is already installed.
    Same { n: u64, hash: String },
    /// A build newer than the published one came from here -- a local
    /// deploy. Never downgrade on our own.
    NewerHere { here: u64, there: u64 },
    Update { n: u64, hash: String },
    /// The package name does not carry `-<n>-<hash>`: without it nothing
    /// can be compared, and swapping binaries blindly is worse than not
    /// swapping.
    NoStamp { name: String },
}

/// Worth swapping?
///
/// Compares by `n` (`git rev-list --count`), which always grows, and not
/// by the tag: the tag is picked by hand and has shown up out of order in
/// every project that uses tags. The hash comes in for the case of equal
/// `n` with a different commit -- which is what a rebase produces.
pub fn decide(n_here: u64, hash_here: &str, package_name: &str) -> Decision {
    let Some((n_there, hash_there)) = stamp_from_name(package_name) else {
        return Decision::NoStamp { name: package_name.to_string() };
    };
    // `abc1234-sujo` is the stamp of a deploy from a dirty tree: it counts
    // as the same commit for comparison purposes, and the difference in
    // `n` is what decides.
    let here_clean = hash_here.strip_suffix("-sujo").unwrap_or(hash_here);
    if n_there == n_here && here_clean == hash_there {
        return Decision::Same { n: n_there, hash: hash_there };
    }
    if n_there < n_here {
        return Decision::NewerHere { here: n_here, there: n_there };
    }
    Decision::Update { n: n_there, hash: hash_there }
}

/// The `n` and `build` the live daemon's `/healthz` returns.
pub fn parse_healthz(json: &str) -> Option<(u64, String)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let n = v.get("n")?.as_u64()?;
    let b = v.get("build")?.as_str()?.to_string();
    Some((n, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{
      "tag_name": "v0.1.0",
      "assets": [
        {"name":"SHA256SUMS","url":"https://api.github.com/x/releases/assets/1","size":300},
        {"name":"oxdesk-linux-x86_64-124-abc1234.tar.gz","url":"https://api.github.com/x/releases/assets/2","size":9000000},
        {"name":"oxdesk-mac-arm64-unsigned-124-abc1234.tar.gz","url":"https://api.github.com/x/releases/assets/3","size":8000000},
        {"name":"oxdesk-windows-x86_64-unsigned-124-abc1234.tar.gz","url":"https://api.github.com/x/releases/assets/4","size":7000000}
      ]
    }"#;

    #[test]
    fn parses_the_release_and_finds_each_systems_package() {
        let l = parse(RELEASE).unwrap();
        assert_eq!(l.tag, "v0.1.0");
        assert_eq!(l.assets.len(), 4);
        assert_eq!(pick(&l, System::Linux).unwrap().name, "oxdesk-linux-x86_64-124-abc1234.tar.gz");
        assert_eq!(pick(&l, System::Mac).unwrap().name, "oxdesk-mac-arm64-unsigned-124-abc1234.tar.gz");
        assert_eq!(
            pick(&l, System::Windows).unwrap().name,
            "oxdesk-windows-x86_64-unsigned-124-abc1234.tar.gz"
        );
        assert_eq!(sums(&l).unwrap().bytes, 300);
    }

    /// The Linux prefix is nobody else's prefix. If someone ever renames
    /// the packages, this test fails before the machine downloads the
    /// wrong system's binary.
    #[test]
    fn one_systems_prefix_does_not_match_another() {
        let l = parse(RELEASE).unwrap();
        for s in [System::Linux, System::Mac, System::Windows] {
            let chosen = pick(&l, s).unwrap();
            let others: Vec<&Asset> = l
                .assets
                .iter()
                .filter(|a| a.name.ends_with(".tar.gz") && a.name != chosen.name)
                .collect();
            for o in others {
                assert!(
                    !o.name.starts_with(crate::plan::prefix(s)),
                    "{:?}: {} also matches", s, o.name
                );
            }
        }
    }

    /// The API answers 200 with `{"message":...}` when the token is
    /// missing on a private repo. Mistaking that for "no package" would
    /// send you looking for the problem in CI.
    #[test]
    fn api_error_message_does_not_become_missing_package() {
        let e = parse(r#"{"message":"Not Found","documentation_url":"..."}"#).unwrap_err();
        assert!(e.contains("Not Found"), "{e}");
    }

    #[test]
    fn stamp_comes_from_the_name_with_and_without_extra_field() {
        assert_eq!(stamp_from_name("oxdesk-linux-x86_64-124-abc1234.tar.gz"), Some((124, "abc1234".into())));
        assert_eq!(
            stamp_from_name("oxdesk-windows-x86_64-unsigned-124-abc1234.tar.gz"),
            Some((124, "abc1234".into()))
        );
        assert_eq!(stamp_from_name("oxdesk-linux-x86_64.tar.gz"), None);
        assert_eq!(stamp_from_name("SHA256SUMS"), None);
        // `zzzzzzz` is not hexadecimal: not a commit hash.
        assert_eq!(stamp_from_name("oxdesk-linux-x86_64-124-zzzzzzz.tar.gz"), None);
    }

    #[test]
    fn decides_by_n_and_breaks_ties_by_hash() {
        let newer = "oxdesk-linux-x86_64-124-abc1234.tar.gz";
        assert_eq!(decide(124, "abc1234", newer), Decision::Same { n: 124, hash: "abc1234".into() });
        assert_eq!(decide(120, "0000000", newer), Decision::Update { n: 124, hash: "abc1234".into() });
        assert_eq!(decide(130, "0000000", newer), Decision::NewerHere { here: 130, there: 124 });
        // Same `n`, different commit: a rebase rewrote history. Swapping
        // is right -- what is published is the truth.
        assert_eq!(decide(124, "0000000", newer), Decision::Update { n: 124, hash: "abc1234".into() });
    }

    /// `-sujo` is how `deploy.sh` stamps a build from a dirty tree.
    /// Without this case, a machine that just got a local deploy would
    /// download the same commit from the Release for "different hash", in
    /// a loop.
    #[test]
    fn dirty_stamp_counts_as_the_same_commit() {
        assert_eq!(
            decide(124, "abc1234-sujo", "oxdesk-linux-x86_64-124-abc1234.tar.gz"),
            Decision::Same { n: 124, hash: "abc1234".into() }
        );
    }

    /// Never swap without knowing what is being swapped.
    #[test]
    fn no_stamp_does_not_update() {
        assert!(matches!(decide(1, "x", "oxdesk-linux-x86_64.tar.gz"), Decision::NoStamp { .. }));
    }

    #[test]
    fn reads_sums_with_path_and_binary_mode() {
        let sums = "\
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  dist/oxdesk-linux-x86_64-124-abc1234.tar.gz
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad *oxdesk
junk that is not a sum
";
        assert_eq!(
            sum_for(sums, "oxdesk-linux-x86_64-124-abc1234.tar.gz").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(sum_for(sums, "oxdesk").unwrap().len(), 64);
        assert_eq!(sum_for(sums, "does-not-exist"), None);
        assert_eq!(lines(sums).len(), 2, "the garbage line does not count");
    }

    #[test]
    fn parses_healthz() {
        let j = r#"{"ok":true,"build":"abc1234","n":124,"host":"x"}"#;
        assert_eq!(parse_healthz(j), Some((124, "abc1234".into())));
        assert_eq!(parse_healthz("not json"), None);
        assert_eq!(parse_healthz(r#"{"ok":true}"#), None);
    }
}
