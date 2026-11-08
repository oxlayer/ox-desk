# oxdesk — domain knowledge

The [`specs/`](../specs/) describe **what we built and why**. This is
what doesn't fit in a spec: the order of operations on a new machine, and
the map of the screen paths.

- [[new-computer]] — the order to set up a machine from scratch
- [[screen-paths]] — who captures, who decodes, who injects

There used to be a `machines` page here, a table of three hostnames, three
user accounts and three SSH aliases. It had no generic version, so it was
deleted rather than anonymized; the one rule inside it — never write an IP
— lives in [`specs/007`](../specs/007-device-routing.md). See
[`.claude/rules/no-personal-data`](../.claude/rules/no-personal-data.md).

The inviolable rules are in [`.claude/rules/`](../.claude/rules/) and
the procedures in [`.claude/skills/`](../.claude/skills/).
