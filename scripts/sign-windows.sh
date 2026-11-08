#!/usr/bin/env bash
# Signs an .exe with Authenticode, from here, before it reaches Windows.
#
# Why here and not there: Defender removed the raw `oxdesk.exe` the moment
# it was written, TWICE -- there was no time to sign it on the other side.
# Signing before copying takes the race out of the picture.
#
# Why it objected, honestly: an unsigned executable with no reputation,
# freshly created, that creates a scheduled task at logon, runs remote
# commands and reads an input device in raw mode. Detected as
# `Trojan:Win32/Bearfoos.A!ml` -- the `!ml` suffix is a model's guess, not
# a known-malware signature. The silhouette really is similar.
#
#   usage:  bash scripts/sign-windows.sh path/to/oxdesk.exe
#
# The private key lives in ~/.config/oxdesk/signing (not versioned, 600).
# The public certificate is imported into Windows' Trusted Root and
# Trusted Publishers stores -- it is valid only on that machine.
set -euo pipefail
export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"

EXE="${1:?usage: sign-windows.sh <file.exe>}"
DIR="$HOME/.config/oxdesk/signing"
[ -f "$DIR/key.pem" ] || { echo "no key in $DIR -- see specs/008" >&2; exit 1; }

TMP="$(mktemp -u "${EXE%.exe}.assinado.XXXX.exe")"
# `-t` timestamps it: the signature survives the certificate's expiry,
# and it is what a normal binary has. If the TSA does not answer, sign
# without a timestamp instead of failing the build.
#
# `timeout 25` is MANDATORY, not zeal. The fallback below only fires when
# the first attempt FAILS -- and a TSA that does not answer does not fail,
# it hangs. Without the timeout the deploy sat 13 minutes in
# `osslsigncode` waiting for timestamp.digicert.com, with not one log line
# saying what was happening.
if timeout 25 osslsigncode sign \
      -certs "$DIR/cert.pem" -key "$DIR/key.pem" \
      -h sha256 -n "oxdesk control plane" \
      -t http://timestamp.digicert.com \
      -in "$EXE" -out "$TMP" >/dev/null 2>&1; then
  echo "  signed with timestamp"
else
  osslsigncode sign \
      -certs "$DIR/cert.pem" -key "$DIR/key.pem" \
      -h sha256 -n "oxdesk control plane" \
      -in "$EXE" -out "$TMP" >/dev/null
  echo "  signed WITHOUT timestamp (the TSA did not answer in 25s)"
fi
mv "$TMP" "$EXE"

# `-CAfile` with OUR certificate, and the result is CHECKED.
#
# Without it, `osslsigncode verify` builds the chain against the system
# store (`/etc/ssl/certs/ca-certificates.crt`), which of course does not
# contain our self-signed certificate -- so it printed
# `Signature verification: failed` on every single deploy while the
# digests matched perfectly. A line that always says the same thing is not
# a check, it is decoration, and this one said "failed" in a deploy log for
# weeks ([[../.claude/rules/verify-what-arrived]]).
#
# The question worth asking is "is this signed by OUR key and intact", and
# with the right CA the same binary answers `Signature verification: ok`.
if saida=$(osslsigncode verify -CAfile "$DIR/cert.pem" "$EXE" 2>&1); then
  printf '%s\n' "$saida" | grep -E 'Signature verification|Subject:|Timestamp time' | sed 's/^/  /'
else
  echo "ERROR: the signature did not verify against $DIR/cert.pem" >&2
  printf '%s\n' "$saida" | tail -20 >&2
  exit 1
fi
