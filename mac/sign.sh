#!/usr/bin/env bash
# Signs a binary with the local identity, if it exists.
#
# Silent and NOT fatal when the identity does not exist: the machine keeps
# working with an ad-hoc binary, it just loses the screen permission on
# every rebuild. The deploy is what turns that into a failure, by checking
# the result -- nothing is brought down here.
set -uo pipefail

# The identity's NAME and the keychain's FILENAME still say `logi`, on
# purpose: both are baked into the certificate's designated requirement, and
# recreating it to rename them would make macOS discard the Screen Recording
# grant -- which only the machine's owner can give back, by hand, in the UI.
# The password is a different matter: it was the literal `logi` in this file
# and in create-identity.sh (see .claude/rules/no-secrets-in-source.md).
NAME="logi local signing"
KC="$HOME/Library/Keychains/logi.keychain-db"
PASSFILE="$HOME/.config/oxdesk/keychain-password"
TARGET="${1:?usage: sign.sh <path>}"

if [ ! -f "$KC" ]; then
  echo "no keychain $KC; run create-identity.sh"
  exit 0
fi

# Always unlock: it is cheap, idempotent, and covers someone having locked
# the keychain along the way. The password lives in one file, read at
# runtime, because a secret duplicated across three files is three chances
# to drift -- and it had already drifted.
if [ -s "$PASSFILE" ]; then
  security unlock-keychain -p "$(cat "$PASSFILE")" "$KC" 2>/dev/null || true
else
  echo "no $PASSFILE: the keychain stays locked and codesign will fail; run create-identity.sh"
fi

# Explicit `--keychain`: without it codesign walks the search list and may
# find an old copy in the login keychain, refusing on ambiguity.
if codesign --force --sign "$NAME" --keychain "$KC" --timestamp=none "$TARGET" 2>&1; then
  echo "signed: $(basename "$TARGET")"
  codesign -dv "$TARGET" 2>&1 | grep -E "Authority|Signature" | sed "s/^/  /"
else
  echo "WARNING: failed to sign $(basename "$TARGET"); staying ad-hoc"
fi
