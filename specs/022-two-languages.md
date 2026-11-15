# 022 — Two languages

> Written for: whoever adds a string to the panel or the wizard, and
> whoever reviews that change.

The panel and the setup wizard show their copy in **English or Brazilian
Portuguese**. The code keeps speaking English: every string is written in
English where it is used, and the Portuguese lives in one catalog keyed by
that English text. This spec says what is translated and what is not, how
the translation reaches the screen, what keeps it complete, and the case
against each part.

## Why

oxdesk was written in Portuguese for one person, then moved to English
when it became open source ([[../.claude/rules/english-only]]). That move
was right for everything a contributor reads, and it took something away
from people who would rather read their screen in Portuguese. Two
languages give that back without reopening the mix the rule closed.

## What is translated, and what is not

| Translated | Not translated, on purpose |
|---|---|
| The panel (`static/index.html`) | Logs, API error bodies, CLI output: read by contributors |
| The setup wizard (`static/setup.html`), including the step titles, buttons, explanations and messages the daemon sends | Code boxes in the wizard (fix recipes): they hold commands, and a translated command breaks |
| "What's new" (`CHANGELOG.pt-BR.md`, the same entries as `CHANGELOG.md`) | Data: machine, file, process and device names, paths, roles, addresses |
| Dates in the file browser, formatted in the chosen language | The Electron tray menu (`gui/main.js`), for now |

## How it works

**One catalog, keyed by the English text.** `oxdesk/static/i18n/pt-BR.json`
maps what the code writes to what the screen shows:

- an exact entry: `"Measure again"` → `"Medir de novo"`;
- a template, for text with values in it: `"{} binaries in {}"` →
  `"{} binários em {}"`. `{}` captures in order; the Portuguese side may
  reorder with `{0}`, `{1}`. A template needs at least three literal
  letters, or it would match anything.

**The translator reads the screen, not the code.**
[`static/i18n.js`](../oxdesk/static/i18n.js) loads in `<head>`, after the
catalog and before the page's own scripts, and watches the document with a
`MutationObserver`. Every text node and every `title`, `placeholder` and
`aria-label` is looked up as it appears, so the first paint is already
translated and the pages' code did not change shape. Anything without an
entry stays in English: a missing translation shows as English, never as
a blank.

**Data is fenced off.** An element with `data-no-i18n` is never touched,
and neither is anything inside `code`, `pre`, `script`, `style` or
`textarea`. Every place the panel renders a name, a path, a role, an
address or a device carries the attribute, because a file called
"Settings" must stay "Settings".

**The language.** The first visit follows the browser (`navigator.languages`:
`pt*` is Portuguese, anything else English). The switch in both pages'
footers saves the choice in `localStorage` (`oxdesk.lang`) and reloads:
per machine and per browser, like the rest of the panel's preferences
([[../.claude/rules/config-per-machine]]).

**The daemon serves it**, embedded like the pages
([`src/i18n.rs`](../oxdesk/src/i18n.rs)): `/i18n.js`, `/i18n/catalogs.js`
(the catalog as a script, so it exists before the first paint) and
`/api/changelog?lang=pt-BR`. A test keeps the two changelogs to the same
entries on the same dates; another keeps every catalog entry well formed.

## What keeps it complete

[`scripts/i18n-check.py`](../scripts/i18n-check.py), in `check.sh`,
collects every string a person can read from the source (static text and
attributes in both pages, prose literals in their scripts, the wizard's
titles, labels and explanations, and the messages of every `Check`) and
fails while one has no entry. It prints how many it found, and finding
none is an error ([[../.claude/rules/verify-what-arrived]]).

The source cannot see everything: a string built by concatenation at
runtime, or a value-bearing message, may only exist on a live page. So the
translator also records every English text it shows without an entry
(`oxdeskI18n.missing()`), and a browser pass over every pane lists them.

## Measured (2026-11-15)

On the Windows machine of the reference deployment, with a browser in
`pt-BR` visiting every pane of the panel and the open wizard:

| Pass | English text shown with no entry |
|---|---:|
| Empty catalog | 113 (after removing data) |
| After the first catalog | 7 |
| Final | **0** |

The gate finds 198 strings in the source and all 198 are translated. A
planted untranslated string fails it (`1 missing`, exit 1) and passes once
removed. In English the pages are unchanged: the same copy, and no new
errors (one 404 the panel already had shows in both languages).

## Found on the way

Translating is reading every string, and it found what nothing else had:

- **"What's new" had no dates.** The API's field became `date` in the rename
  of 2026-11-07 and the panel kept reading `data`. Fixed here, and now an
  example in [[../.claude/rules/english-only]].
- **Portuguese left in the panel**, all of it without accents, which is why
  earlier counts missed it: the sidebar ("Favoritos", "Paineis", "Visao
  geral", "Botoes", "Disposicao"), "Tarefas", "Detalhes", "Conectar", "no
  ar", the footer and three wizard messages. Now English, and Portuguese
  through the catalog.
- **Dates followed the browser, not the panel**: a Portuguese browser showed
  Portuguese dates on an English panel. They follow the chosen language.

## Adversarial review

### 1. "Matching on-screen text is fragile; use `t()` calls like everyone else"

The pages are two single files of 1,800 and 550 lines that build most of
their copy in template literals. Wrapping every string in `t()` is about
300 edits across both pages, for the same result.
Matching on screen leaves the code in English and puts the whole
translation in one file a translator can work on alone.

*What survives:* the fragility is real where text is assembled from
fragments ("7" + " of " + "8" + " essentials ready"): each fragment is
translated on its own, and word order cannot change across them. Copy that
reads badly that way should become one string with a template.

### 2. "Something that is not copy will get translated"

Only exact matches and templates are applied, so arbitrary text rarely
matches. Names, paths and roles are fenced with `data-no-i18n`, found by
reading every place the panel renders a value.

*What survives:* a new place that renders a name must carry the attribute,
and nothing checks that automatically. The browser pass shows such a place
as "English with no entry", which is how the current ones were found.

### 3. "It costs time on every update of the page"

Each distinct text is looked up once and cached; the cache is cleared at
4,000 entries because values change on every poll. Not measured with a
profiler: no slowdown was visible in the browser passes.

### 4. "Translate in the daemon instead"

Then the API, the logs and the CLI would carry two languages, and the
English-only rule exists precisely because a contributor has to read
those. Translation is a property of the screen, so it happens on the
screen.

## Not done yet

- The Electron tray menu (`gui/main.js`) is a native menu, not a page, so
  the translator does not reach it.
- A third language is one more catalog file, one entry in `LANGS` in
  `i18n.js` and one in `catalogs_js()`.

## See also

- [[../.claude/rules/english-only]] — written in English, shown in two
  languages
- [[019-one-onboarding]] — the wizard this translates
- [[018-open-source]] — why the code moved to English
