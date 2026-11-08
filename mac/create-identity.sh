#!/usr/bin/env bash
# Creates, once, the local code-signing identity on the Mac.
#
# WHY THIS EXISTS
#
# The Screen Recording permission (TCC) is tied to the binary. For an
# ad-hoc / linker-signed executable -- what `cargo build` produces -- the
# binding is by cdhash, which changes on every build. Measured: grant the
# permission, run a deploy, and the host goes back to hanging exactly as
# before, with no error at all in the log.
#
# With a STABLE identity TCC binds to the designated requirement, and the
# grant survives rebuilds.
#
# WHY A KEYCHAIN OF ITS OWN, AND NOT THE LOGIN ONE
#
# In the login keychain `codesign` asks for authorization through a GUI
# dialog on every signature -- and over SSH there is not even a dialog: it
# fails with `errSecInternalComponent`. Typing the password in the dialog
# fixes ONE operation; it does not stick.
#
# A dedicated keychain, with a password we know, can be unlocked and have
# its partition list set from the command line -- and then `codesign` runs
# without asking, even over SSH. It is the pattern every macOS CI uses.
#
# The password is not a secret on purpose: the keychain holds ONE
# self-signed key whose only power is signing our own binaries so TCC
# recognizes them. It signs nothing another machine trusts.
set -euo pipefail

NAME="logi local signing"
KC="$HOME/Library/Keychains/logi.keychain-db"
KCPASS="logi"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Remove it from the LOGIN keychain, if an earlier attempt left it there:
# two identities with the same name make codesign refuse on ambiguity.
while security find-certificate -c "$NAME" "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1; do
  security delete-certificate -c "$NAME" "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1 || break
done

# IDEMPOTENT, and this matters more than it seems.
#
# Recreating the identity generates a NEW certificate, with a new public
# key -- and the designated requirement changes with it. TCC treats that as
# another program and DISCARDS the Screen Recording grant, which only the
# machine's owner can give back, by hand, in the UI.
#
# In other words: running this script twice costs a trip to Settings.
# That is why it refuses, instead of "ensuring state".
if [ -f "$KC" ] && security find-certificate -c "$NAME" "$KC" >/dev/null 2>&1; then
  echo "the identity '$NAME' already exists in $KC -- nothing to do."
  echo "Recreating it would invalidate the Screen Recording permission already granted."
  echo "To recreate anyway: security delete-keychain \"$KC\" and run again."
  security find-identity -p codesigning "$KC" 2>&1 | tail -2
  exit 0
fi

security delete-keychain "$KC" 2>/dev/null || true
security create-keychain -p "$KCPASS" "$KC"
# No auto-lock: a keychain that locks itself asks for a dialog again in the
# middle of a deploy.
security set-keychain-settings -lut 100000 "$KC"
security unlock-keychain -p "$KCPASS" "$KC"

P12PASS="tmp-$$-$RANDOM"
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$TMP/k.pem" -out "$TMP/c.pem" -subj "/CN=$NAME" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null

# LEGACY format: OpenSSL 3+ exports PKCS12 with a SHA-256 MAC, which the
# Apple keychain cannot read -- and the error blames the password ("MAC
# verification failed, wrong password?") when the problem is the algorithm.
openssl pkcs12 -export -out "$TMP/p.p12" -inkey "$TMP/k.pem" -in "$TMP/c.pem" \
  -passout pass:"$P12PASS" -macalg sha1 -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES 2>/dev/null

security import "$TMP/p.p12" -k "$KC" -P "$P12PASS" -T /usr/bin/codesign -A

# THIS is what removes the dialog. Without the partition list macOS asks
# for GUI authorization on every use of the private key.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KCPASS" "$KC" >/dev/null

# Join the search list without evicting the ones already there.
CURRENT=$(security list-keychains -d user | sed 's/[" ]//g' | tr '\n' ' ')
# shellcheck disable=SC2086
security list-keychains -d user -s $CURRENT "$KC"

echo "identity created in $KC"
security find-identity -p codesigning "$KC" 2>&1 | tail -3
