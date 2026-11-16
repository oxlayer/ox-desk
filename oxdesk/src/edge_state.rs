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

/// A monitor: `(left, top, width, height)` in virtual-desktop pixels.
pub type Rect = (i32, i32, i32, i32);

fn contains(m: &Rect, x: i32, y: i32) -> bool {
    x >= m.0 && x < m.0 + m.2 && y >= m.1 && y < m.1 + m.3
}

/// How far `p` is from an OUTER edge in that direction, per monitor: the
/// edge of the monitor `p` is on, when no other monitor continues past it
/// at that height (or width). `None` when another monitor does, or when
/// `p` is on no monitor.
///
/// The bounding box of all monitors is not the shape of the desktop. On
/// the reference Windows machine a 1920x1080 monitor sits left of a
/// 2560x1440 one: the box's left edge is x = -1920 at every height, but
/// below y = 1080 the cursor stops at x = 0, a real edge the box never
/// reaches -- the bottom of the primary monitor's left side switched to
/// nothing (spec 025 section 3).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn monitor_edge_distance(edge: &str, p: (i32, i32), monitors: &[Rect]) -> Option<i32> {
    let m = monitors.iter().find(|m| contains(m, p.0, p.1))?;
    let (bx, by, dist) = match edge {
        "left" => (m.0 - 1, p.1, p.0 - m.0),
        "right" => (m.0 + m.2, p.1, m.0 + m.2 - 1 - p.0),
        "top" => (p.0, m.1 - 1, p.1 - m.1),
        "bottom" => (p.0, m.1 + m.3, m.1 + m.3 - 1 - p.1),
        _ => return None,
    };
    if monitors.iter().any(|o| contains(o, bx, by)) {
        return None;
    }
    Some(dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference Windows machine, measured: a 1920x1080 monitor left
    /// of the 2560x1440 primary, tops aligned.
    const REF: [Rect; 2] = [(-1920, 0, 1920, 1080), (0, 0, 2560, 1440)];

    #[test]
    fn the_left_edge_follows_the_monitors_not_the_box() {
        assert_eq!(monitor_edge_distance("left", (-1920, 500), &REF), Some(0));
        // Between the two monitors, the left one continues: not an edge.
        assert_eq!(monitor_edge_distance("left", (0, 500), &REF), None);
        // Below the left monitor's bottom, the primary's left side IS the
        // edge -- the dead zone of the bounding box.
        assert_eq!(monitor_edge_distance("left", (0, 1200), &REF), Some(0));
        assert_eq!(monitor_edge_distance("left", (150, 1200), &REF), Some(150));
    }

    #[test]
    fn the_other_sides_and_the_corners() {
        assert_eq!(monitor_edge_distance("right", (2559, 100), &REF), Some(0));
        assert_eq!(monitor_edge_distance("bottom", (100, 1439), &REF), Some(0));
        assert_eq!(monitor_edge_distance("bottom", (-100, 1079), &REF), Some(0));
        assert_eq!(monitor_edge_distance("top", (-1920, 0), &REF), Some(0));
        // Off every monitor: no answer, not a guess.
        assert_eq!(monitor_edge_distance("left", (-100, 1200), &REF), None);
        assert_eq!(monitor_edge_distance("sideways", (0, 0), &REF), None);
    }

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
