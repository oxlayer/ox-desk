#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Every string a person reads in the panel has a translation in each
catalog under oxdesk/static/i18n/ (specs/022).

What it collects, from the source and not from a running page:
  - static text in oxdesk/static/*.html, outside <script> and <style>
  - title, placeholder and aria-label attributes there
  - string and template literals in their scripts that look like prose
    (start with a capital letter and contain a space, or are listed in
    the catalog already): the panel builds most of its copy in JS
  - the wizard's step titles and button labels in oxdesk/src/setup.rs
  - the messages its checks send to the page (Check::ok/missing/error/
    when in oxdesk/src/setup_*.rs), as templates: every {...} becomes {}

usage: python3 scripts/i18n-check.py            # gate: exit 1 on a gap
       python3 scripts/i18n-check.py --list     # every string it found
       python3 scripts/i18n-check.py --missing  # only the untranslated ones

A gate that cannot say how many things it checked is not a check
(.claude/rules/verify-what-arrived.md): it prints the counts, and zero
strings found is an error.
"""
import html
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATIC = os.path.join(ROOT, "oxdesk", "static")
SRC = os.path.join(ROOT, "oxdesk", "src")
CATALOGS = os.path.join(STATIC, "i18n")

PLACEHOLDER = re.compile(r"\{[^{}]*\}")


def template(s):
    """Rust and JS interpolations both become `{}`; entities become the
    characters the screen shows."""
    s = re.sub(r"\$\{[^}]*\}", "{}", s)      # JS ${...}
    return html.unescape(PLACEHOLDER.sub("{}", s))


def is_prose(s):
    s = html.unescape(s).strip()
    if len(s) < 2 or not re.search(r"[A-Za-z]{2,}", s):
        return False
    # Code that a loose literal match picks up: comments, calls, arrows.
    if s.startswith("//") or re.search(r"\.\w+\($|=>|\);?$", s):
        return False
    # A template that is mostly holes ("{}: {}") would match anything.
    if len(re.findall(r"[A-Za-z]", PLACEHOLDER.sub("", s))) < 3:
        return False
    if re.fullmatch(r"[\w.\-/:#%()\[\]{}]+", s):          # identifiers, paths, css
        return False
    if s.startswith(("http", "/", ".", "#", "--", "oxdesk-")) and " " not in s:
        return False
    return True


# What can come right before a `/` that starts a regex literal rather than
# a division: an operator, an opening bracket, or nothing.
REGEX_BEFORE = set("(,=:[!&|?{};+-*%<>~^") | {""}


def js_literals(js):
    """Every string literal in a script, as written: quoted strings whole,
    template literals with each `${...}` turned into `{}`. Nested templates
    inside a `${...}` come out too.

    A tokenizer and not a regex. The regex this replaced paired quotes
    across a whole script, and one template literal holding another
    (`${a ? `x` : 'y'}`) or a quote inside a regex literal (`/[&<>"]/g`)
    shifted every pairing after it: on 2026-11-15 it was reading markup
    attributes as strings and had stopped seeing the panel's JS sentences
    at all, so untranslated copy passed the gate.
    """
    out = []
    i, n = 0, len(js)
    prev = ""  # last significant character outside literals and comments

    def template_at(i):
        """Reads a template literal starting after its backtick. Returns its
        text with holes as `{}` and the index after the closing backtick."""
        text = []
        while i < n:
            c = js[i]
            if c == "\\":
                text.append(js[i:i + 2])
                i += 2
            elif c == "`":
                return "".join(text), i + 1
            elif c == "$" and js[i + 1:i + 2] == "{":
                text.append("{}")
                i = code_until_brace(i + 2)
            else:
                text.append(c)
                i += 1
        return "".join(text), n

    def code_until_brace(i):
        """Skips the code of a `${...}` hole, collecting the literals inside
        it, and returns the index after its closing brace."""
        depth = 1
        while i < n and depth:
            c = js[i]
            if c in "'\"":
                j = quoted_end(i)
                out.append(js[i + 1:j - 1])
                i = j
            elif c == "`":
                s, i = template_at(i + 1)
                out.append(s)
            else:
                depth += {"{": 1, "}": -1}.get(c, 0)
                i += 1
        return i

    def quoted_end(i):
        q, j = js[i], i + 1
        while j < n and js[j] != q and js[j] != "\n":
            j += 2 if js[j] == "\\" else 1
        return j + 1

    while i < n:
        c = js[i]
        if js.startswith("//", i):
            i = js.find("\n", i)
            i = n if i < 0 else i
        elif js.startswith("/*", i):
            i = js.find("*/", i + 2)
            i = n if i < 0 else i + 2
        elif c in "'\"":
            j = quoted_end(i)
            out.append(js[i + 1:j - 1])
            i, prev = j, "a"
        elif c == "`":
            s, i = template_at(i + 1)
            out.append(s)
            prev = "a"
        elif c == "/" and prev in REGEX_BEFORE:
            # A regex literal: skip it whole, character classes included.
            j, in_class = i + 1, False
            while j < n and js[j] != "\n":
                if js[j] == "\\":
                    j += 2
                    continue
                if js[j] == "[":
                    in_class = True
                elif js[j] == "]":
                    in_class = False
                elif js[j] == "/" and not in_class:
                    break
                j += 1
            i, prev = j + 1, "a"
        else:
            if not c.isspace():
                prev = c if not (c.isalnum() or c in "_$") else "a"
                # `return /x/` is a regex too: a keyword ends in a letter, so
                # look back for it.
                if prev == "a" and js[max(0, i - 6):i + 1].endswith("return"):
                    prev = ""
            i += 1
    return out


def html_strings(path):
    t = open(path, encoding="utf-8").read()
    out = set()
    body = re.sub(r"<(script|style)\b.*?</\1>", "", t, flags=re.S | re.I)
    body = re.sub(r"<!--.*?-->", "", body, flags=re.S)
    for m in re.finditer(r">([^<>]+)<", body):
        s = html.unescape(" ".join(m.group(1).split()))
        if is_prose(s) and "{" not in s:
            out.add(s)
    for m in re.finditer(r'\b(?:title|placeholder|aria-label)="([^"]+)"', body):
        if is_prose(m.group(1)):
            out.add(html.unescape(" ".join(m.group(1).split())))
    m = re.search(r"<title>([^<]+)</title>", t)
    if m and is_prose(m.group(1)):
        out.add(m.group(1).strip())
    # Script literals: prose only. Code strings (selectors, keys, URLs)
    # rarely start with a capital letter AND contain a space.
    for sm in re.finditer(r"<script\b[^>]*>(.*?)</script>", t, flags=re.S | re.I):
        for raw in js_literals(sm.group(1)):
            if "<" in raw:  # markup in a literal: take its text parts
                for part in re.findall(r">([^<>]+)<", raw):
                    part = " ".join(part.split())
                    if is_prose(part) and re.match(r"[A-Z]", part):
                        out.add(template(part))
                continue
            s = " ".join(raw.split())
            if is_prose(s) and re.match(r"[A-Z]", s) and " " in s:
                out.add(template(s))
    for cm in re.finditer(r"\w+(?:\.length)?\s*\+\s*'( [a-z][^'<>]*)'", t):
        if is_prose("x" + cm.group(1)):
            # A trailing space means a value follows: `n + ' rows - ' + path`.
            tail = "{}" if cm.group(1) != cm.group(1).rstrip() else ""
            out.add(("{}" + cm.group(1)).rstrip() + (" " + tail if tail else ""))
    return out


def rust_text(s):
    """A Rust string literal as the program prints it."""
    s = re.sub(r"\\\n\s*", "", s)          # line continuation
    return s.replace('\\"', '"').replace("\\\\", "\\")


def rust_strings():
    out = set()
    setup = open(os.path.join(SRC, "setup.rs"), encoding="utf-8").read()
    # `\\[\s\S]`, not `\\.`: a `\` before a line break is a continuation,
    # and `.` does not cross a line.
    for m in re.finditer(r'\b(?:title|label|why):\s*"((?:[^"\\]|\\[\s\S])*)"', setup):
        s = rust_text(m.group(1))
        if s.strip():
            out.add(s)
    lit = r'"((?:[^"\\]|\\[\s\S])*)"'
    for name in sorted(os.listdir(SRC)):
        if not re.fullmatch(r"setup_\w+\.rs", name):
            continue
        src = open(os.path.join(SRC, name), encoding="utf-8").read()
        src = re.sub(r"#\[cfg\(test\)\].*", "", src, flags=re.S)  # tests are not the page
        src = re.sub(r"(?m)^\s*//.*$", "", src)                     # nor are comments
        for cm in re.finditer(r"Check::(?:ok|missing|error|when|warn)\(", src):
            depth, i = 1, cm.end()
            while i < len(src) and depth:
                depth += {"(": 1, ")": -1}.get(src[i], 0)
                i += 1
            call = src[cm.end():i - 1]
            strings = re.findall(lit, call)
            for s in strings[1:]:                 # the first one is the step id
                s = rust_text(s)
                if is_prose(s) and not re.fullmatch(r"[a-z\-]+", s):
                    out.add(template(s))
    return out


def main():
    # Portuguese and arrows must print on a Windows console too.
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    found = set()
    for name in sorted(os.listdir(STATIC)):
        if name.endswith(".html"):
            found |= html_strings(os.path.join(STATIC, name))
    found |= rust_strings()
    if not found:
        print(f"i18n: found 0 strings to translate (cwd {os.getcwd()}): the scan is blind")
        return 1
    if "--list" in sys.argv:
        print("\n".join(sorted(found)))
        return 0

    catalogs = sorted(f for f in os.listdir(CATALOGS) if f.endswith(".json")) if os.path.isdir(CATALOGS) else []
    if not catalogs:
        print(f"i18n: no catalog in {CATALOGS}")
        return 1
    failures = 0
    for cat in catalogs:
        data = json.load(open(os.path.join(CATALOGS, cat), encoding="utf-8"))
        keys = {template(k) for k in data}
        missing = sorted(s for s in found if s not in keys)
        bad = [k for k, v in data.items()
               if not isinstance(v, str) or not v.strip()
               or len(PLACEHOLDER.findall(template(k))) != len(PLACEHOLDER.findall(template(v)))]
        if "--missing" in sys.argv:
            print("\n".join(missing))
            continue
        print(f"i18n {cat}: {len(found)} strings found, {len(found) - len(missing)} translated, "
              f"{len(missing)} missing, {len(bad)} malformed")
        for s in missing[:15]:
            print(f"    missing: {s}")
        for k in bad[:15]:
            print(f"    malformed (empty, or a different number of {{}}): {k}")
        failures += bool(missing or bad)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
