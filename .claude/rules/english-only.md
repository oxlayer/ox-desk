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

On 2026-11-08, only one crate still had Portuguese text:

| Crate | Files with accented text | Accented lines |
|---|---:|---:|
| `oxdesk` | 15 | ~241 |
| `stream` | 4 | ~0 |
| `ota`, `hidpp`, `agent`, `gui` | 0 | 0 |

So this is bounded, mostly user-facing strings and `say!`/`gripe!`
messages in `oxdesk/src`. The rule exists to keep it from growing back
while that work is staged ([[../../specs/018-open-source]], step 6).

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
