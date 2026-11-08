#!/usr/bin/env bash
# Signs a binary with the local identity, if it exists.
#
# Silent and NOT fatal when the identity does not exist: the machine keeps
# working with an ad-hoc binary, it just loses the screen permission on
# every rebuild. The deploy is what turns that into a failure, by checking
# the result -- nothing is brought down here.
set -uo pipefail
NAME="logi local signing"
KC="$HOME/Library/Keychains/logi.keychain-db"
TARGET="${1:?usage: sign.sh <path>}"

if [ ! -f "$KC" ]; then
  echo "no keychain $KC; run create-identity.sh"
  exit 0
fi

# Always unlock: it is cheap, idempotent, and covers someone having locked
# the keychain along the way.
security unlock-keychain -p logi "$KC" 2>/dev/null || true

# Explicit `--keychain`: without it codesign walks the search list and may
# find an old copy in the login keychain, refusing on ambiguity.
if codesign --force --sign "$NAME" --keychain "$KC" --timestamp=none "$TARGET" 2>&1; then
  echo "signed: $(basename "$TARGET")"
  codesign -dv "$TARGET" 2>&1 | grep -E "Authority|Signature" | sed "s/^/  /"
else
  echo "WARNING: failed to sign $(basename "$TARGET"); staying ad-hoc"
fi
