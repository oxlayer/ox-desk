# 023 — Switching settings: the shortcut, the edge, and saying what they do

> Written for: whoever changes how a person moves the keyboard and mouse
> between machines, and whoever reviews that change.

The ways to move the keyboard and mouse to another machine -- the screen
edge, the keyboard shortcut and "the other devices follow the leader" --
live in **Settings › Switching machines**, as switches that look like
switches, each with a line that says what it is doing right now. The
shortcut's keys are a choice. This spec says why each of those is so, and
the case against it.

## What was wrong (2026-11-15)

Four reports in one session, each from looking at the panel:

| Report | What was really happening |
|---|---|
| The edge and reconciler pills "look like settings to be toggled" | They were status, drawn as rounded pills next to each other: the shape of a control. The real toggles were small checkboxes in the Layout header. |
| "The shortcut should be configurable; no one knows that both Ctrl switches machines" | The gesture existed only in a code comment and a spec. Nothing on screen named it. |
| "I tried the edge, it does not work well" | Turned on from the panel at 20:39, the edge did nothing: the watcher had quit at boot because it was off then, and only a restart at 20:43 started it. The panel's note even said "restart it", which is a bug asking the person to work around it. Then it was disarmed because the keyboard and mouse were on another machine -- and nothing on screen said so. |
| Cards of different heights | A remote machine has a Connect block, this one does not, and the grid sized each card to its content. |

## What changed

- **Settings › Switching machines** holds three rows: *Switch at the screen
  edge*, *Keyboard shortcut*, *Devices follow the leader*. Each is a
  switch; the shortcut adds the pair, the follower adds the leader.
- **The edge says what it is doing.** The watchers report a state
  (`oxdesk/src/edge_state.rs`), `/api/layout` passes it on as
  `border_state`, and the panel turns it into a sentence: off, no
  neighbor, watching (with the edges), waiting because the devices are on
  another machine, just arrived, the input is on another machine, or not
  available here and why.
- **The edge starts when it is turned on.** Both watchers
  (`border.rs`, `border_win.rs`) wait for an edge instead of returning
  without one. Changing the layout while the watcher runs still needs a
  restart; the note in Layout says so.
- **The shortcut's pair is a choice:** both Ctrl (default), both Shift,
  both Alt (`[input.hotkey] keys`). The pair is named once, in
  `config::CHORDS`, with its evdev codes.
- **The way back uses the same pair.** `kvm::send` passes it to
  `oxdesk-kvm` as `--rescue <a>,<b>`; the keys that sent the input away
  are the keys that bring it home. The flag is passed only when the pair
  is not the default, so binaries from before it keep working.
- **The Overview names the gesture**: a one-line tip with the pair in use.
- **The cards show status as text**, with a colored dot, and stretch to
  the same height per row, with "Details" at the same line.

## The case against

**"Any key combination would be more flexible."** It would also be a way
to lose the keyboard. While the input is away every key goes to the other
machine; a combination that includes a letter or a function key is stolen
from every program on both. A pair of the same modifier, left and right,
is held by no program and typed by no text -- which is why hardware KVM
switches use it. Three such pairs cover the real conflict (a program that
needs both Ctrl), and a free-form recorder would need a way back that does
not depend on the recorder being right.

**"A rescue that also always accepts both Ctrl would be safer."** It would
take both Ctrl away from the person who chose another pair precisely
because something on the other machine needs both Ctrl. The panel says
which pair is in use; one rule is easier to keep in mind than two.

**"The state line duplicates the log."** The log is where a contributor
looks; the person at the screen looks at the switch. Both reports above
were answered by the log and invisible on the screen.

## How it is checked

- `config::chord_codes`, `input::parse_chord`, `input::chord_held` and
  `hotkey::chord_key_from_vk` are pure and tested on every machine.
- `hotkey::tests::the_daemon_chords_are_the_ones_the_config_names` pins the
  panel's choices to keys the Windows reader can see.
- `scripts/i18n-check.py` now reads the panel's JavaScript with a
  tokenizer. The regex it replaced lost track of quotes at the first
  nested template literal and had stopped seeing the panel's JS sentences;
  a planted sentence inside a nested template now fails the gate.

## See also

- [`005`](005-input-switch.md) — the LAN KVM and why the way back must not
  depend on the network
- [`024`](024-windows-kvm-source.md) — the Windows half of the shortcut
- [`022`](022-two-languages.md) — why every new sentence above has a
  translation in the same change
