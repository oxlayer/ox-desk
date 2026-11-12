#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# Rotates the self-signed code-signing key used to sign the Windows
# binaries before they are copied over (scripts/sign-windows.sh, and
# specs/008 for why signing happens on this side).
#
# usage:  bash scripts/new-signing-cert.sh            # stage a new pair
#         bash scripts/new-signing-cert.sh --activate # make it the signing key
#         bash scripts/new-signing-cert.sh --cn 'yourproject (local signing)' --org 'Your Org'
#
# It existed only as an openssl command somebody ran once, in no file --
# which is how the certificate ended up carrying a name from before the
# project was renamed, in a language the project does not write in, inside
# every Windows binary published for five years. A recipe that is not a
# script is a recipe that drifts.
#
# The certificate is self-signed and trusted on exactly one machine: it
# proves "the same key that signed this signed the last one", nothing
# about who we are. That is what stops Defender racing the file to disk
# (specs/008); it is not a public trust anchor and must not be read as one.
#
# ## Why two steps and not one
#
# Generating the key and using it are separated because the window between
# them is dangerous, and the first version of this script opened it. A new
# key is a publisher Windows has never heard of: sign with it before the
# certificate is in that machine's stores and the next deploy hands
# Defender an unknown-publisher binary that creates a scheduled task at
# logon -- the exact race specs/008 was written about, which has already
# eaten the .exe twice and the scheduled task once.
#
# So `--stage` touches nothing that signs, and `--activate` refuses until
# it has ASKED WINDOWS whether the certificate arrived. A local file is
# not the claim; the claim is that the other machine trusts it
# (.claude/rules/verify-what-arrived).
set -euo pipefail
cd "$(dirname "$0")/.." || exit 1

CN='oxdesk (local signing)'
ORG='oxlayer'
DAYS=1826 # five years and the leap day, matching what it replaces
BITS=3072 # as the certificate it replaces; Authenticode is happy either way
MODE='stage'
FORCE=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --activate) MODE='activate'; shift ;;
    --cn) CN="$2"; shift 2 ;;
    --org) ORG="$2"; shift 2 ;;
    --days) DAYS="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    *) echo "new-signing-cert: unknown argument: $1" >&2; exit 2 ;;
  esac
done

DIR="$HOME/.config/oxdesk/signing"
mkdir -p "$DIR"
chmod 700 "$DIR"

thumb_of() {
  openssl x509 -in "$1" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g'
}

# -- activate ----------------------------------------------------------
if [ "$MODE" = 'activate' ]; then
  [ -f "$DIR/cert.new.pem" ] || {
    echo "nothing staged in $DIR -- run this without --activate first" >&2
    exit 1
  }
  want="$(thumb_of "$DIR/cert.new.pem")"

  # Fail closed, and say which half is missing. Without the target we
  # cannot ask, and "cannot ask" must never read as "yes" -- that is the
  # whole shape of the bug this ordering exists to prevent.
  # shellcheck source=/dev/null
  [ -f scripts/targets.sh ] && . scripts/targets.sh
  if [ -z "${OXDESK_WIN_SSH:-}" ]; then
    echo 'no OXDESK_WIN_SSH: cannot ask Windows whether it trusts the staged' >&2
    echo 'certificate, so this refuses rather than guess. See targets.example.sh.' >&2
    exit 1
  fi

  echo "asking $OXDESK_WIN_SSH whether it trusts $want ..."
  found=$(ssh -o ConnectTimeout=10 -o BatchMode=yes "$OXDESK_WIN_SSH" \
    "powershell -NoProfile -Command \"(Get-ChildItem Cert:\\LocalMachine\\Root,Cert:\\LocalMachine\\TrustedPublisher,Cert:\\CurrentUser\\Root,Cert:\\CurrentUser\\TrustedPublisher -ErrorAction SilentlyContinue | Where-Object Thumbprint -eq '$want').Count\"" \
    2>/dev/null | tr -dc '0-9') || true

  if [ -z "$found" ] || [ "$found" -eq 0 ]; then
    cat >&2 <<EOF
$OXDESK_WIN_SSH does not have $want in any of its four stores.

Activating now would sign the next deploy with a publisher that machine
has never seen, and Defender has already deleted an unsigned oxdesk.exe
twice (specs/008). Import it there first -- the commands are printed by
the staging run, and cert.new.cer is the file to copy:

    certutil -addstore -f Root             cert.new.cer
    certutil -addstore -f TrustedPublisher cert.new.cer
    certutil -user -addstore -f Root             cert.new.cer
    certutil -user -addstore -f TrustedPublisher cert.new.cer

Then run this again. Nothing has been changed.
EOF
    exit 1
  fi
  echo "  found in $found store(s)"

  # The outgoing pair is kept, never deleted: `osslsigncode verify
  # -CAfile` needs the certificate that actually signed a binary, so
  # discarding it would make every already-published build unverifiable.
  stamp="$(date -u +%Y%m%d)"
  kept=0
  for f in key.pem cert.pem cert.cer; do
    if [ -f "$DIR/$f" ]; then
      mv "$DIR/$f" "$DIR/${f%.*}.$stamp.${f##*.}"
      kept=1
    fi
  done
  mv "$DIR/cert.new.pem" "$DIR/cert.pem"
  mv "$DIR/key.new.pem" "$DIR/key.pem"
  mv "$DIR/cert.new.cer" "$DIR/cert.cer"
  chmod 600 "$DIR/key.pem"
  chmod 644 "$DIR/cert.pem" "$DIR/cert.cer"

  echo
  echo "now signing with:"
  openssl x509 -in "$DIR/cert.pem" -noout -subject -dates
  if [ "$kept" = 1 ]; then
    echo "the previous pair is kept as *.$stamp.* -- leave the old certificate"
    echo "trusted on the machines that have it, or what it signed stops verifying"
  fi
  echo
  echo "next: bash scripts/deploy.sh --skip-mac, then confirm on Windows that"
  echo "Get-AuthenticodeSignature reports Valid with thumbprint $want"
  exit 0
fi

# -- stage -------------------------------------------------------------
if [ -f "$DIR/cert.new.pem" ] && [ "$FORCE" = 0 ]; then
  echo "a pair is already staged ($(thumb_of "$DIR/cert.new.pem")):" >&2
  openssl x509 -in "$DIR/cert.new.pem" -noout -subject >&2
  echo 'activate it with --activate, or replace it with --force' >&2
  exit 1
fi

# `-noenc`: osslsigncode runs unattended inside deploy.sh, so a passphrase
# would be a passphrase typed into a script or stored next to the key.
# What protects it is the 600 and the 700 above, plus never leaving here.
openssl req -x509 -newkey "rsa:$BITS" -noenc \
  -keyout "$DIR/key.new.pem" -out "$DIR/cert.new.pem" \
  -days "$DAYS" -sha256 \
  -subj "/CN=$CN/O=$ORG" \
  -addext 'keyUsage=critical,digitalSignature' \
  -addext 'extendedKeyUsage=critical,codeSigning' \
  -addext 'basicConstraints=critical,CA:FALSE' \
  2>/dev/null

chmod 600 "$DIR/key.new.pem"
chmod 644 "$DIR/cert.new.pem"

# DER, because `certutil -addstore` on Windows wants a .cer. Only the
# public half crosses the network -- the private key has never been on
# that machine and does not need to be (specs/008).
openssl x509 -in "$DIR/cert.new.pem" -outform DER -out "$DIR/cert.new.cer"
chmod 644 "$DIR/cert.new.cer"

echo 'staged, and nothing signs with it yet:'
openssl x509 -in "$DIR/cert.new.pem" -noout -subject -dates
echo "sha1 thumbprint: $(thumb_of "$DIR/cert.new.pem")"
echo '              (this is what Windows calls the Thumbprint)'
cat <<EOF

Copy $DIR/cert.new.cer to the Windows machine and
import it into four stores -- the same four the certificate it replaces
lives in. The LocalMachine pair needs an elevated shell:

    certutil -addstore -f Root             cert.new.cer
    certutil -addstore -f TrustedPublisher cert.new.cer
    certutil -user -addstore -f Root             cert.new.cer
    certutil -user -addstore -f TrustedPublisher cert.new.cer

Then:  bash scripts/new-signing-cert.sh --activate

which checks that machine for the thumbprint above and refuses if it is
not there. Until then the old key keeps signing and deploys stay safe.
EOF
