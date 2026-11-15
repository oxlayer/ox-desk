// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two languages for what a person reads: English and Brazilian Portuguese
//! (specs/022-two-languages.md).
//!
//! The daemon and the pages keep writing English, which is what logs, the
//! API and the CLI speak. `static/i18n.js` translates what reaches the
//! screen, with the catalogs served from here. Embedded with `include_str!`
//! for the same reason as the pages: what a machine shows is what that
//! machine runs.

/// The translator. Loaded in `<head>`, after [`catalogs_js`].
pub const SCRIPT: &str = include_str!("../static/i18n.js");

/// English text -> Portuguese, keyed by the English the code writes.
/// `{}` marks a value in a template; scripts/i18n-check.py keeps every
/// string a person reads in here.
const PT_BR: &str = include_str!("../static/i18n/pt-BR.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    PtBr,
}

/// The language a request asks for: `lang=pt-BR` (any `pt...`) in the
/// query. Anything else, including no parameter, is English.
pub fn lang_of(url: &str) -> Lang {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("lang=") {
            if v.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("pt")) {
                return Lang::PtBr;
            }
        }
    }
    Lang::En
}

/// The catalogs as a script that defines `window.OXDESK_CATALOG`, so the
/// page has them before its first paint. JSON is a JavaScript literal, so
/// the file is used as it is.
pub fn catalogs_js() -> String {
    format!("window.OXDESK_CATALOG={{\"pt-BR\":{}}};\n", PT_BR.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn catalog() -> BTreeMap<String, String> {
        serde_json::from_str(PT_BR).expect("static/i18n/pt-BR.json is not an object of strings")
    }

    fn holes(s: &str) -> usize {
        // `{}` and `{0}`..`{9}`, the two forms the translator fills.
        let b = s.as_bytes();
        (0..b.len())
            .filter(|&i| b[i] == b'{' && (b.get(i + 1) == Some(&b'}') || (b.get(i + 2) == Some(&b'}') && b[i + 1].is_ascii_digit())))
            .count()
    }

    #[test]
    fn the_catalog_is_well_formed() {
        let c = catalog();
        assert!(c.len() > 50, "only {} entries: the catalog did not load what it should", c.len());
        for (en, pt) in &c {
            assert!(!pt.trim().is_empty(), "empty translation for {en:?}");
            assert_eq!(holes(en), holes(pt), "a different number of {{}} in {en:?} -> {pt:?}");
        }
    }

    #[test]
    fn the_query_picks_the_language() {
        assert_eq!(lang_of("/api/changelog?lang=pt-BR"), Lang::PtBr);
        assert_eq!(lang_of("/api/changelog?x=1&lang=pt"), Lang::PtBr);
        assert_eq!(lang_of("/api/changelog?lang=en"), Lang::En);
        assert_eq!(lang_of("/api/changelog"), Lang::En);
        assert_eq!(lang_of("/api/changelog?lang="), Lang::En);
        assert_eq!(lang_of("/api/changelog?xlang=pt-BR"), Lang::En);
    }

    #[test]
    fn the_script_defines_the_catalog() {
        let js = catalogs_js();
        assert!(js.starts_with("window.OXDESK_CATALOG={\"pt-BR\":{"));
        let first = catalog().into_iter().next().unwrap().0;
        assert!(js.contains(&serde_json::to_string(&first).unwrap()));
    }
}
