// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What's new, embedded in the binary.
//!
//! `include_str!` and not a file on disk on purpose: what the panel shows
//! has to be what THAT machine is running. Reading from disk, a machine
//! with an old binary would show news it does not have -- which is exactly
//! the mistake the build number exists to avoid.

use serde::Serialize;

const TEXT: &str = include_str!("../CHANGELOG.md");
/// The same entries in Brazilian Portuguese (specs/022). A test keeps the
/// two files to the same entries, on the same dates, in the same order.
const TEXT_PT_BR: &str = include_str!("../CHANGELOG.pt-BR.md");

/// The wire names are the Rust names: the panel reads `date`, `title`
/// and `description`.
#[derive(Debug, Serialize, PartialEq)]
pub struct Entry {
    pub date: String,
    pub title: String,
    pub description: String,
}

/// The entries in the language the panel asked for; English otherwise.
pub fn entries_for(lang: crate::i18n::Lang) -> Vec<Entry> {
    parse(match lang {
        crate::i18n::Lang::PtBr => TEXT_PT_BR,
        crate::i18n::Lang::En => TEXT,
    })
}

#[cfg(test)]
fn entries() -> Vec<Entry> {
    parse(TEXT)
}

/// Reads `## <date> · <title>` followed by the description lines.
fn parse(text: &str) -> Vec<Entry> {
    let mut v = Vec::new();
    let mut current: Option<Entry> = None;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(e) = current.take() {
                v.push(e);
            }
            // The separator is `·`; without it, everything becomes the
            // title and the date stays empty -- better than dropping the
            // entry.
            let (date, title) = match heading.split_once('·') {
                Some((d, t)) => (d.trim().to_string(), t.trim().to_string()),
                None => (String::new(), heading.trim().to_string()),
            };
            current = Some(Entry { date, title, description: String::new() });
        } else if let Some(e) = current.as_mut() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if !e.description.is_empty() {
                e.description.push(' ');
            }
            e.description.push_str(t);
        }
    }
    if let Some(e) = current {
        v.push(e);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_file_parses() {
        let v = entries();
        assert!(v.len() >= 5, "too few entries: {}", v.len());
        // If the format broke, the panel would show empty titles silently
        // -- this assert is what makes that loud.
        for e in &v {
            assert!(!e.title.is_empty(), "entry without a title: {e:?}");
            assert!(!e.description.is_empty(), "entry without a description: {e:?}");
            assert!(e.date.starts_with("20"), "odd date: {e:?}");
        }
    }

    #[test]
    fn both_languages_carry_the_same_entries() {
        // A translation that drifts (an entry added to one file only, a
        // date typed differently) would show a different history per
        // language, and nobody reads both to notice.
        let en = parse(TEXT);
        let pt = parse(TEXT_PT_BR);
        assert_eq!(en.len(), pt.len(), "entry count differs between the two changelogs");
        for (a, b) in en.iter().zip(&pt) {
            assert_eq!(a.date, b.date, "dates out of step: {:?} vs {:?}", a.title, b.title);
            assert!(!b.title.is_empty() && !b.description.is_empty(), "empty pt-BR entry: {b:?}");
        }
        // And the Portuguese file is really the Portuguese one.
        assert_ne!(en[0].title, pt[0].title);
    }

    #[test]
    fn the_panel_gets_the_language_it_asked_for() {
        use crate::i18n::Lang;
        assert_eq!(entries_for(Lang::En), parse(TEXT));
        assert_eq!(entries_for(Lang::PtBr), parse(TEXT_PT_BR));
    }

    #[test]
    fn multi_line_description_becomes_one() {
        // Markdown wraps lines by width; the UI wants a paragraph.
        let v = entries();
        let multi = v.iter().find(|e| e.description.len() > 80);
        assert!(multi.is_some(), "expected at least one long description");
        assert!(!multi.unwrap().description.contains('\n'));
    }
}
