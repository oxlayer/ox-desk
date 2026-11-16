// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the edge watcher is doing right now, for the panel.
//!
//! Spec: ../../specs/023-shortcut.md
//!
//! The config says whether the edge is ON; it cannot say whether it WORKS.
//! On 2026-11-15 the panel showed the edge on while the watcher had quit at
//! boot (it was off then, and nothing started it later), and again while it
//! was on and disarmed because the keyboard and mouse were on another
//! machine. Both read as "the edge is broken", and the log was the only
//! place that said which. The watcher now reports here, and `/api/layout`
//! passes it on.
//!
//! The states are wire values: the panel turns each into a sentence.

use std::sync::Mutex;

/// A state and, for some, a detail (the edges being watched, or why the
/// watcher cannot run here).
static STATE: Mutex<(&str, Option<String>)> = Mutex::new(("starting", None));

/// - `starting`: the watcher has not reported yet.
/// - `off`: turned off in the config.
/// - `no-neighbor`: on, but no machine that can take the input sits next to
///   this one in the layout.
/// - `watching`: armed; the detail lists the edges.
/// - `away`: disarmed because the keyboard and mouse are on another machine.
/// - `arriving`: they just arrived; it arms after a few seconds.
/// - `sent`: the input is on another machine through the LAN KVM; the
///   detail names it.
/// - `unavailable`: the watcher cannot run here; the detail says why
///   (`macos`, `session`, `another-watcher`, `input-capture`).
pub fn set(state: &'static str, detail: Option<String>) {
    if let Ok(mut g) = STATE.lock() {
        *g = (state, detail);
    }
}

pub fn get() -> (String, Option<String>) {
    STATE
        .lock()
        .map(|g| (g.0.to_string(), g.1.clone()))
        .unwrap_or_else(|_| ("starting".into(), None))
}

/// The edges as one line: `left -> ubuntu-host, right -> mac-host`.
pub fn summary(edges: &[(String, String)]) -> String {
    edges.iter().map(|(e, t)| format!("{e} -> {t}")).collect::<Vec<_>>().join(", ")
}

/// How far a point is from one edge of an area `(left, top, width,
/// height)`, in pixels; `None` for a name that is not an edge. The edge
/// watcher wakes the other side's screen host when the cursor gets close.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn distance_to_edge(edge: &str, p: (i32, i32), area: (i32, i32, i32, i32)) -> Option<i32> {
    let (x, y, w, h) = area;
    Some(match edge {
        "left" => p.0 - x,
        "right" => x + w - 1 - p.0,
        "top" => p.1 - y,
        "bottom" => y + h - 1 - p.1,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_is_measured_from_the_edge_that_was_named() {
        // Two monitors, the left one at negative x.
        let area = (-1920, 0, 4480, 1440);
        assert_eq!(distance_to_edge("left", (-1920, 500), area), Some(0));
        assert_eq!(distance_to_edge("left", (-1800, 500), area), Some(120));
        assert_eq!(distance_to_edge("right", (2559, 500), area), Some(0));
        assert_eq!(distance_to_edge("bottom", (0, 1400), area), Some(39));
        assert_eq!(distance_to_edge("top", (0, 10), area), Some(10));
        assert_eq!(distance_to_edge("middle", (0, 10), area), None);
    }

    #[test]
    fn the_summary_lists_every_edge() {
        let e = vec![("left".to_string(), "a".to_string()), ("right".to_string(), "b".to_string())];
        assert_eq!(summary(&e), "left -> a, right -> b");
        assert_eq!(summary(&[]), "");
    }
}
