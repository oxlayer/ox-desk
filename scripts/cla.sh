#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# Decides whether a pull request's author has agreed to the inbound terms
# in CONTRIBUTING.md -- the grant that keeps a future relicence possible
# (specs/021). This script and `.cla/signed` are the whole mechanism: a
# line in a file, added by the contributor, in a commit they signed off.
#
# usage:  bash scripts/cla.sh <login> <author-association> <user-type>
#              [--signed FILE]   the ledger to read      (default .cla/signed)
#              [--base FILE]     the base branch's copy, to verify the
#                                pull request only APPENDS to it
#         bash scripts/cla.sh --self-test
#
# Why not `cla-assistant/github-action`, which is what one reaches for:
# it has been **archived and read-only since March 2026**, and it needs
# `pull_request_target` -- a trigger that hands THIS repository's write
# token to a job started by a fork. specs/018 rules that trigger out in
# writing, and an unmaintained third-party action holding a write token is
# the worst combination on the menu. So the signature arrives as a commit
# rather than as a bot comment, which is also the better record: it is
# authored by the account making the claim, and it is in the diff a
# maintainer approves instead of in a database.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

# Bump when the terms in CONTRIBUTING.md change in substance. Old
# signatures then stop counting, which is the point -- nobody agreed to
# terms that did not exist on the day they signed.
CLA_VERSION=1

# Push access is itself an agreement: a collaborator was invited by hand
# and an organisation member is under one already. GitHub states that
# relationship in the event payload, so the test is structural -- it needs
# no token, no API call, and **no maintainer's name in this file**, which
# matters because this is a script a stranger runs
# (.claude/rules/no-personal-data.md).
owes_signature() {
  local association="$1" kind="$2"
  case "$kind" in
    Bot) return 1 ;;
  esac
  case "$association" in
    OWNER | MEMBER | COLLABORATOR) return 1 ;;
  esac
  return 0
}

# GitHub logins are letters, digits and single hyphens, nothing else.
# Checking the shape before the value reaches a regex is what stops a
# login carrying a metacharacter into the match below.
valid_login() {
  printf '%s' "$1" | grep -qE '^[A-Za-z0-9](-?[A-Za-z0-9])*$'
}

has_signed() {
  local login="$1" file="$2"
  [ -f "$file" ] || return 1
  # Anchored at both ends and the version pinned: without `^...$`, `ann`
  # signing would also sign for `annie`, and without the version an
  # agreement to version 1 would silently cover version 2.
  grep -qE "^${login} ${CLA_VERSION} [0-9]{4}-[0-9]{2}-[0-9]{2}$" "$file"
}

# Every signature in the base has to survive verbatim. Comparing line by
# line rather than counting deletions means the failure can name what went
# missing, and an edited line fails as the deletion it is.
#
# Comments are exempt, and only comments: they hold no signature, so
# guarding them buys nothing and costs a false refusal the first time
# somebody fixes a typo in the file's own header. Turning a signature INTO
# a comment is still caught, because the signature line is then the thing
# that has gone missing.
missing_from() {
  local base="$1" now="$2" line
  [ -f "$base" ] || return 0
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    case "$line" in '#'*) continue ;; esac
    if ! grep -qxF "$line" "$now"; then
      printf '%s\n' "$line"
      return 1
    fi
  done <"$base"
  return 0
}

refuse() {
  local login="$1"
  cat <<EOF
This pull request cannot be merged until its author has agreed to the
inbound terms. Add one line to .cla/signed, in this pull request:

    $login $CLA_VERSION $(date -u +%Y-%m-%d)

and commit it with \`git commit -s\`. The line is the record, your commit
is the signature, and the sign-off is the DCO we ask for as well.

What you are agreeing to is in CONTRIBUTING.md, under "What you need to
know about the licence". In short: you keep your copyright, and oxlayer
may distribute your contribution under other terms in future, including
commercial ones. Nothing already published stops being AGPL -- a licence
change can only ever apply going forward.

If that is not acceptable to you, say so in the pull request and do not
sign. We would rather close it than merge something we cannot license.
EOF
}

# -- self-test ---------------------------------------------------------
# specs/021 says it out loud about the licence check, and it is just as
# true here: a check that has never failed is a check nobody has tested.
# Both directions are exercised, because the expensive failure is the one
# where the gate stays silent.
self_test() {
  local tmp fails=0 out
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT

  printf 'ann %s 2026-11-12\nocto-cat %s 2026-11-12\nold 0 2026-01-01\n' \
    "$CLA_VERSION" "$CLA_VERSION" >"$tmp/signed"

  t() {
    local name="$1" want="$2"
    shift 2
    out="$("$@" 2>&1)"
    local got=$?
    if [ "$got" = "$want" ]; then
      printf '  ok    %s\n' "$name"
    else
      printf '  FAIL  %s (wanted exit %s, got %s) %s\n' "$name" "$want" "$got" "$out"
      fails=$((fails + 1))
    fi
  }

  t 'a signature counts'                   0 has_signed ann "$tmp/signed"
  t 'an unsigned author does not'          1 has_signed stranger "$tmp/signed"
  t 'a login is not a prefix match'        1 has_signed annie "$tmp/signed"
  t 'a hyphen is a legal login'            0 has_signed octo-cat "$tmp/signed"
  t 'an older version does not count'      1 has_signed old "$tmp/signed"
  t 'a missing ledger signs nobody'        1 has_signed ann "$tmp/none"

  t 'an outside contributor owes one'      0 owes_signature CONTRIBUTOR User
  t 'a first-timer owes one'               0 owes_signature FIRST_TIME_CONTRIBUTOR User
  t 'a stranger owes one'                  0 owes_signature NONE User
  t 'the owner owes nothing'               1 owes_signature OWNER User
  t 'an org member owes nothing'           1 owes_signature MEMBER User
  t 'a collaborator owes nothing'          1 owes_signature COLLABORATOR User
  t 'a bot owes nothing'                   1 owes_signature NONE Bot

  t 'a login shape is checked'             0 valid_login octo-cat
  t 'a regex metacharacter is refused'     1 valid_login 'a.*'
  t 'a shell metacharacter is refused'     1 valid_login 'a;b'
  t 'an empty login is refused'            1 valid_login ''

  cp "$tmp/signed" "$tmp/plus"
  printf 'new %s 2026-11-13\n' "$CLA_VERSION" >>"$tmp/plus"
  t 'appending is allowed'                 0 missing_from "$tmp/signed" "$tmp/plus"
  grep -v '^ann ' "$tmp/signed" >"$tmp/minus"
  t 'removing a line is not'               1 missing_from "$tmp/signed" "$tmp/minus"
  sed 's/^ann .*/ann 99 2026-11-12/' "$tmp/signed" >"$tmp/edited"
  t 'editing a line is not'                1 missing_from "$tmp/signed" "$tmp/edited"
  t 'a ledger new in the PR is allowed'    0 missing_from "$tmp/none" "$tmp/plus"
  printf '# a header\nann %s 2026-11-12\n' "$CLA_VERSION" >"$tmp/withdoc"
  printf '# a HEADER, retyped\nann %s 2026-11-12\n' "$CLA_VERSION" >"$tmp/redoc"
  t 'a comment may be rewritten'           0 missing_from "$tmp/withdoc" "$tmp/redoc"
  printf '# a header\n# ann %s 2026-11-12\n' "$CLA_VERSION" >"$tmp/commented"
  t 'commenting OUT a signature is not'    1 missing_from "$tmp/withdoc" "$tmp/commented"

  if [ "$fails" -eq 0 ]; then
    echo "cla: all checks behave in both directions"
    return 0
  fi
  echo "cla: $fails check(s) wrong"
  return 1
}

# -- main --------------------------------------------------------------
if [ "${1:-}" = '--self-test' ]; then
  self_test
  exit $?
fi

login="${1:-}"
association="${2:-}"
kind="${3:-}"
signed='.cla/signed'
base=''
shift 3 2>/dev/null || true
while [ "$#" -gt 0 ]; do
  case "$1" in
    --signed) signed="$2"; shift 2 ;;
    --base) base="$2"; shift 2 ;;
    *) echo "cla: unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$login" ] || [ -z "$association" ]; then
  echo 'usage: bash scripts/cla.sh <login> <author-association> <user-type>' >&2
  exit 2
fi

if ! valid_login "$login"; then
  echo "cla: refusing to match on \"$login\": not the shape of a GitHub login" >&2
  exit 2
fi

# Order matters: the tamper check runs for everyone, including the people
# who owe no signature. A maintainer deleting a line is the same loss of
# the record as a stranger doing it, and a check that trusts whoever it
# exempts protects nothing.
if [ -n "$base" ]; then
  if ! gone="$(missing_from "$base" "$signed")"; then
    echo "cla: this pull request removes or rewrites a line that was already in .cla/signed:" >&2
    echo "       $gone" >&2
    echo '     The ledger is append-only. Restore the line and add yours below it.' >&2
    exit 1
  fi
fi

if ! owes_signature "$association" "$kind"; then
  echo "cla: $login is $association here, which is already an agreement -- nothing to sign"
  exit 0
fi

if has_signed "$login" "$signed"; then
  echo "cla: $login agreed to the terms, version $CLA_VERSION"
  exit 0
fi

refuse "$login"
exit 1
