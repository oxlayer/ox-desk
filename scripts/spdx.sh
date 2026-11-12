#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# Every file that holds logic says under which licence it is published.
#
# usage:  bash scripts/spdx.sh          # checks; lists what is missing
#         bash scripts/spdx.sh --fix    # writes the header into those files
#
# Why a check and not a one-time sweep: a relicence is an audit, and
# "which files, under which terms" has to be answerable mechanically
# rather than by reading `git log` (specs/021). A sweep answers it once.
# This answers it on every run of check.sh, including for the file added
# next week by someone who never read this script.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

YEAR=2026
HOLDER='oxlayer'

# The set is deliberate. A file that holds logic carries the header; a
# declarative file does not, and there is a reason per kind:
#
# - `Cargo.toml` already declares `license =`, which is the answer tooling
#   actually reads for a crate. A second declaration is a second thing to
#   keep in sync, and the two disagreeing is worse than one existing.
# - `.desktop`, `.service`, `.rules` and `.plist` are parsed by systemd,
#   udev and launchd, which are strict about what they accept, and they
#   hold no logic to attribute.
# - Markdown is documentation: LICENSE and NOTICE cover the repository,
#   and 55 headers would put boilerplate at the top of every page.
#
# This list is what the check enforces. Extending the project to a new
# kind of source file means adding it here -- not remembering to paste a
# header by hand.
files() {
  git ls-files '*.rs' '*.sh' '*.ps1' '*.js' '*.swift' '*.html' 'linux/oxdesk-panel'
}

# hidpp is MIT and stays separate on purpose: it is the piece we want
# other projects to take, and permissive means no constraint in either
# direction (specs/021).
license_of() {
  case "$1" in
    hidpp/*) echo 'MIT' ;;
    *) echo 'AGPL-3.0-or-later' ;;
  esac
}

comment_style() {
  case "$1" in
    *.rs | *.js | *.swift) echo 'slash' ;;
    *.html) echo 'html' ;;
    *) echo 'hash' ;; # .sh, .ps1, linux/oxdesk-panel
  esac
}

header() {
  local id
  id="$(license_of "$1")"
  case "$(comment_style "$1")" in
    slash) printf '// SPDX-FileCopyrightText: %s %s\n// SPDX-License-Identifier: %s\n' "$YEAR" "$HOLDER" "$id" ;;
    hash) printf '# SPDX-FileCopyrightText: %s %s\n# SPDX-License-Identifier: %s\n' "$YEAR" "$HOLDER" "$id" ;;
    html) printf '<!-- SPDX-FileCopyrightText: %s %s -->\n<!-- SPDX-License-Identifier: %s -->\n' "$YEAR" "$HOLDER" "$id" ;;
  esac
}

# Line 1 is not always ours to take: a shebang has to stay on it or the
# file stops being executable, and a doctype has to stay first or the
# browser renders the page in quirks mode. In Rust nothing is reserved --
# a leading `#![cfg(...)]` is an inner attribute, not a shebang, and a
# comment is allowed above it.
keeps_first_line() {
  case "$1" in
    *.rs) return 1 ;;
    *.html) head -1 "$1" | grep -qi '^[[:space:]]*<!doctype' && return 0 ;;
    *) head -1 "$1" | grep -q '^#!' && return 0 ;;
  esac
  return 1
}

has_header() {
  local id
  id="$(license_of "$1")"
  # First lines only: a header further down is a header nobody reads, and
  # an identifier that appears in prose should not pass the check. The
  # identifier has to be the RIGHT one, so an AGPL header inside hidpp/
  # fails here instead of quietly misdeclaring the licence.
  head -6 "$1" | grep -q "SPDX-License-Identifier: $id" &&
    head -6 "$1" | grep -q 'SPDX-FileCopyrightText:'
}

insert() {
  local f="$1" tmp keep next
  tmp="$(mktemp)"
  keep=0
  keeps_first_line "$f" && keep=1
  {
    [ "$keep" = 1 ] && head -1 "$f"
    header "$f"
    # A blank line between the header and the code, unless there is one
    # already: the header is metadata and should not read as the first
    # line of the module documentation below it.
    next="$(tail -n +$((keep + 1)) "$f" | head -1)"
    [ -n "$next" ] && echo
    tail -n +$((keep + 1)) "$f"
  } >"$tmp"
  # `cat >` and not `mv`: this keeps the inode, and with it the executable
  # bit that every script in scripts/ depends on.
  cat "$tmp" >"$f"
  rm -f "$tmp"
}

missing=()
while IFS= read -r f; do
  [ -f "$f" ] || continue
  has_header "$f" || missing+=("$f")
done < <(files)

total="$(files | wc -l | tr -d ' ')"

# Zero files is never the truth about this repository, so it is the
# check failing rather than the repository being clean -- and the message
# it used to print was "all 0 files declare a licence", exit 0. Measured
# on 2026-11-11: with the `cd` above failing, `git ls-files` writes "not a
# git repository" to stderr, returns nothing, and the licence gate passes
# vacuously. Guarding the `cd` fixes that one cause; this fails closed
# whatever the cause, including an export with no .git at all.
if [ "$total" -eq 0 ]; then
  echo 'no source files found at all: this check is broken, not the repo' >&2
  echo "  cwd: $PWD" >&2
  exit 1
fi

if [ "${1:-}" = '--fix' ]; then
  if [ "${#missing[@]}" -eq 0 ]; then
    echo "nothing to write: all $total files already declare a licence"
    exit 0
  fi
  for f in "${missing[@]}"; do insert "$f"; done
  echo "header written into ${#missing[@]} of $total file(s)"
  exit 0
fi

if [ "${#missing[@]}" -eq 0 ]; then
  echo "all $total files declare a licence"
  exit 0
fi

printf 'no SPDX header, or the wrong one (%d of %d):\n' "${#missing[@]}" "$total"
printf '  %s\n' "${missing[@]}"
echo 'to write them: bash scripts/spdx.sh --fix'
exit 1
