# Every word a contributor reads is English

Code, comments, commit messages, specs, rules, skills, log lines, error
messages, UI strings, test names, file names. One language, no exceptions
by layer.

## Why this changed

It used to be a deliberate mix: identifiers and comments in English, and
anything the person on screen read in Portuguese, because that person was
one person. Open source removes the premise. A contributor who cannot read
a log line cannot debug the thing that printed it, and an error message is
the part of a product that gets read under pressure.

## The state, measured

Counting accented lines is the wrong measurement, and it was the first one
taken. Portuguese without accents (`nao`, `esta`, `faltam`, `igual`,
`sujo`) is wider than Portuguese with them, and **the hardest part is not
prose**: it is the wire format. A `serde(rename = "estado")` is one line
with a reader on the other side, so it costs more to move than a paragraph
of comments.

Done (2026-11-08): the wizard end to end, and the whole API contract --
16 `serde` renames, the request fields, the state values, `hidpp`'s JSON
keys and its battery words. Zero Portuguese identifiers remain in any
crate.

Left, in [[../../specs/018-open-source]] step 6: the panel's visible copy
in `index.html`, 29 CLI flags and subcommands, the OTA's state vocabulary
with the build stamp's `-sujo`, and the variable names in `scripts/`.

## Move a wire name with its reader, or not at all

A field name, a route, a step id, an enum value on the wire and a CLI flag
are each a contract. Rename one and leave its consumer, and nothing fails
to compile: the daemon writes `state` while the page reads `estado`, and
the page shows a blank where a word used to be. Every such rename goes in
the **same commit** as every reader of it, and the readers are found by
enumerating them, not by grepping for what comes to mind.

The one that bit: the copy queue's `andando`/`erro` reach the page as CSS
class names. A rename that touched only the daemon would have left failed
rows unstyled -- which reads as a design choice, not a bug.

## Translating, not transliterating

A message's job is to tell someone what happened and what to do. Three
that carry their weight and should survive the translation intact:

- `oxdesk-ota`'s refusal names the missing dylib **and** the fix, because
  without the second half the symptom has no cause.
- The KVM's fatal lines are read by the person who just lost their
  keyboard; they say which key releases it.
- `/healthz` answers a build stamp, not `ok: true` alone, because the
  question behind it is always "which code is running".

Translate the meaning. If a phrase only made sense in Portuguese, rewrite
it rather than producing English that says less.

## What stays as it is

- **`hidpp`** keeps its name: it is Logitech's protocol and the name is
  theirs.
- **Historical quotes** inside specs stay verbatim when the point is what
  a tool printed that day — in a code block, with the translation beside
  it if it matters.
- **Commit messages already written** are not rewritten. History is a
  record, not a surface.

## See also

- [[no-personal-data]] — the other half of making this readable by
  strangers
- [[../../specs/018-open-source]] — the plan and the order of work
