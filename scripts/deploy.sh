#!/usr/bin/env bash
# Builds, signs and ships the daemon to the three machines, and checks
# that all three ended up on the SAME commit.
#
# It exists because syncing by hand failed: the Mac spent a whole day on
# an old binary and the one who noticed was the user, not me ("I don't
# see the new stuff on the mac"). Three targets, two of them remote, one
# needing a signature -- that is not a job for memory.
#
#   usage:  bash scripts/deploy.sh [--skip-mac]
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"

# The Windows key has a passphrase, and the GNOME agent holds it. In an
# SSH session this variable comes empty: Windows accepts the key, the
# signature never happens, and Windows answers `Permission denied`.
if [ -z "${SSH_AUTH_SOCK:-}" ] && [ -S "/run/user/$(id -u)/gcr/ssh" ]; then
  export SSH_AUTH_SOCK="/run/user/$(id -u)/gcr/ssh"
fi

SKIP_MAC=0
[ "${1:-}" = "--skip-mac" ] && SKIP_MAC=1

# WHERE this sends things. Not in the repo: a script someone runs must not
# carry someone else's machines (.claude/rules/no-personal-data.md). Copy
# scripts/targets.example.sh to scripts/targets.sh and fill it in.
TARGETS="$(dirname "$0")/targets.sh"
# shellcheck source=/dev/null
[ -f "$TARGETS" ] && . "$TARGETS"
WIN="${OXDESK_WIN_SSH:-}"
MAC="${OXDESK_MAC_SSH:-}"
if [ -z "$WIN" ]; then
  echo "no target for the Windows machine." >&2
  echo "  cp scripts/targets.example.sh scripts/targets.sh   and fill it in" >&2
  echo "  (or export OXDESK_WIN_SSH=<ssh alias>)" >&2
  exit 1
fi
# No Mac configured is the same as --skip-mac: the alternative is fifteen
# steps failing one by one against an empty host name.
[ -z "$MAC" ] && SKIP_MAC=1

# `desconhecido` and `-sujo` are part of the build stamp that oxdesk parses
# (oxdesk/build.rs, setup_comum.rs): they stay as they are.
BUILD="$(git rev-parse --short HEAD 2>/dev/null || echo desconhecido)"
git diff --quiet 2>/dev/null || BUILD="$BUILD-sujo"
export OXDESK_BUILD="$BUILD"
# Incremental number: tells WHICH machine is behind, not just that they
# differ. It travels along because the Mac's copy has no .git.
# BUILD_BASE + the commit count, because the count RESTARTED.
#
# The branch history was squashed on 2026-11-08, so `rev-list --count` went
# from 240 to 1 -- and the build number is what the updater compares. With
# `n_here = 1` against the published package's `n_there = 229`, `decide()`
# in ota/src/release.rs returns `Update` and every machine would install
# OLDER code over itself on the next timer tick. The base keeps the number
# monotonic across the rewrite.
OXDESK_BUILD_N="$(( $(cat BUILD_BASE 2>/dev/null || echo 0) + $(git rev-list --count HEAD 2>/dev/null || echo 0) ))"
export OXDESK_BUILD_N
echo "== build #$OXDESK_BUILD_N $BUILD =="

failures=0

# An "ok" step has to mean nothing went wrong -- neither our explicit
# `ERROR` nor the shell complaining. (`ERRO` also matches `ERROR`.)
#
# I broke the screen-host step with a backtick inside an expanding
# heredoc (bash ran `r` as a command) and the step said **ok**: the error
# went to stderr, outside the variable the test read. A step that does
# not capture stderr cannot fail, and a step that cannot fail is not a
# check -- it is decoration.
bad() {
  printf '%s' "$1" | grep -qE 'ERRO|command not found|No such file|unbound variable|cannot'
}

step() { printf '  %-44s' "$1"; }
ok()    { echo "ok"; }
fail()  { echo "FAILED"; echo "$1" | tail -6 | sed 's/^/      /'; failures=$((failures+1)); }

# Runs a PowerShell script on Windows without going through shell quoting.
# -EncodedCommand has no quoting at all; quotes and parentheses inside
# -Command break the parse, and PowerShell validates the WHOLE script
# before running, so one bad line at the end means none of it runs.
psrun() {
  local enc; enc=$(iconv -f UTF-8 -t UTF-16LE | base64 -w0)
  timeout 180 ssh -o BatchMode=yes "$WIN" "powershell -NoProfile -EncodedCommand $enc" 2>&1 |
    grep -av 'post-quantum\|store now\|openssh.com/pq\|^\*\* \|#< CLIXML\|<Objs Version' | tr -d '\r'
}

# ---------- ubuntu ----------
step "ubuntu: build"
if out=$(cargo build --release --manifest-path oxdesk/Cargo.toml 2>&1); then ok; else fail "$out"; fi
step "ubuntu: install and restart"
# `pkill -x oxdesk` before the restart: `systemctl restart` only kills the
# copy systemd created. An orphan `oxdesk` (the GUI used to spawn a child
# of its own in a boot race) keeps the port, the new daemon dies with
# `Address already in use` and the machine serves an old binary while
# saying `ok`. See .claude/rules/one-supervisor-per-daemon.md.
if out=$( { install -m755 oxdesk/target/release/oxdesk ~/.local/bin/oxdesk &&
            { pkill -x oxdesk || true; } &&
            systemctl --user restart oxdesk.service &&
            for _ in $(seq 10); do
              sleep 1
              curl -s --max-time 2 "localhost:8787/healthz" | grep -q "$BUILD" && break
            done &&
            curl -s --max-time 2 "localhost:8787/healthz" | grep -q "$BUILD" ||
            echo "ERROR: Ubuntu /healthz did not confirm $BUILD"; } 2>&1 ); then
  if bad "$out"; then fail "$out"; else ok; fi
else fail "$out"; fi

step "ubuntu: menu entry"
# The panel has no window of its own: it is a page served by the daemon,
# so without this entry there is NO way to reach it from the desktop --
# only the tray, and the tray is gone whenever the AppIndicator extension
# is off. A `.desktop` entry pointing at a personal path was deleted in
# specs/018 step 2 and nothing generic replaced it; this is the generic
# replacement.
#
# `@BIN@` is substituted here and not written in the repo, because a
# `.desktop` file cannot expand `$HOME` and a hardcoded /home/<name> is
# exactly what step 2 removed.
if out=$( { install -m755 linux/oxdesk-panel ~/.local/bin/oxdesk-panel &&
            mkdir -p ~/.local/share/applications &&
            sed "s|@BIN@|$HOME/.local/bin|g" linux/oxdesk.desktop \
              > ~/.local/share/applications/oxdesk.desktop &&
            { desktop-file-validate ~/.local/share/applications/oxdesk.desktop ||
              echo "ERROR: invalid .desktop"; } &&
            { update-desktop-database ~/.local/share/applications 2>/dev/null || true; }; } 2>&1 ); then
  if bad "$out"; then fail "$out"; else ok; fi
else fail "$out"; fi

step "ubuntu: updater (oxdesk-ota)"
# The one that downloads the Release, checks the sum, swaps and reverts.
# Goes to ALL THREE machines -- on Windows it only checks and reports
# (specs/015).
if out=$(cargo build --release --manifest-path ota/Cargo.toml 2>&1 &&
         install -m755 ota/target/release/oxdesk-ota ~/.local/bin/oxdesk-ota 2>&1); then ok; else fail "$out"; fi

step "ubuntu: screen and KVM (oxdesk-stream)"
# The host runs on this machine when someone asks for its screen; the
# client is installed too, for when it becomes the one watching.
#
# `oxdesk-kvm` has to be here because `oxdesk` launches it by name
# when the tray sends input to another machine. It was left out of the
# first version and the button failed with "nao consegui executar
# oxdesk-kvm" -- binary built, never installed.
if out=$(cargo build --release --manifest-path stream/Cargo.toml 2>&1 &&
         install -m755 stream/target/release/oxdesk-host ~/.local/bin/ 2>&1 &&
         install -m755 stream/target/release/oxdesk-view ~/.local/bin/ 2>&1 &&
         install -m755 stream/target/release/oxdesk-bridge ~/.local/bin/ 2>&1 &&
         install -m755 stream/target/release/oxdesk-kvm ~/.local/bin/ 2>&1 &&
         mkdir -p ~/.local/state 2>&1); then ok; else fail "$out"; fi

# ---------- windows ----------
# Removes every leftover `<name>.new.exe` on Windows.
#
# The Windows SFTP server REFUSES to overwrite: `Overwriting an existing
# file is not supported`, with no filename in the message. Six steps below
# upload to a fixed `.new.exe`, so one file left by an interrupted deploy
# blocks the next one -- and because the message names nothing, it sent me
# chasing three different steps for a single cause.
#
# Cleaning once at the top of the Windows section was not enough: whatever
# leaves the file does it DURING the run. So the cleanup lives next to the
# operation it protects, called by `send` immediately before each upload.
#
# No variable is interpolated into this heredoc, on purpose: an expanding
# heredoc is how this file broke once already.
clean_leftovers() {
  { cat <<'PS1' | psrun; } 2>&1
$d = Join-Path $env:LOCALAPPDATA 'oxdesk'
$leftover = @()
foreach ($n in 'oxdesk','oxdesk-agent','oxdesk-ota','oxdesk-view','oxdesk-bridge','oxdesk-host') {
  $p = Join-Path $d ($n + '.new.exe')
  if (Test-Path $p) {
    Get-Process -Name $n -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    try { Remove-Item -Force $p -ErrorAction Stop } catch { }
    if (Test-Path $p) { $leftover += $p }
  }
}
if ($leftover.Count) { Write-Output ('ERROR: still in the way: ' + ($leftover -join ', ')); exit 1 }
PS1
}

# Sends a signed binary to Windows as `<name>.new.exe`, clearing the way
# first and naming the file when it cannot.
send() {
  local src="$1" name="$2" cleaned scp_out code
  cleaned=$(clean_leftovers)
  if printf '%s' "$cleaned" | grep -q 'ERROR'; then printf '%s\n' "$cleaned"; return 1; fi
  scp_out=$(timeout 180 scp -q "$src" "$WIN:AppData/Local/oxdesk/$name.new.exe" 2>&1)
  code=$?
  [ "$code" -eq 0 ] && return 0
  # The upload failed, and `scp`'s message names no file -- which is how one
  # cause looked like three unrelated failures. So the step reports WHAT IS
  # THERE, from the machine itself, instead of leaving the reader to guess.
  printf '%s\n' "$scp_out"
  printf 'ERRO: o upload de %s failed (exit %s). Estado em %%LOCALAPPDATA%%\\oxdesk:\n' "$name" "$code"
  { cat <<'PS1' | psrun; } 2>&1 | sed 's/^/    /'
$d = Join-Path $env:LOCALAPPDATA 'oxdesk'
Get-ChildItem $d -Filter '*.exe' -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 8 |
  ForEach-Object { $_.LastWriteTime.ToString('HH:mm:ss') + '  ' + $_.Length.ToString().PadLeft(9) + '  ' + $_.Name }
Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'oxdesk*' } |
  ForEach-Object { 'rodando: ' + $_.Name + ' pid ' + $_.Id }
PS1
  return 1
}

step "windows: build"
if out=$(cargo build --release --target x86_64-pc-windows-gnu --manifest-path oxdesk/Cargo.toml 2>&1); then ok; else fail "$out"; fi

EXE=oxdesk/target/x86_64-pc-windows-gnu/release/oxdesk.exe
SIGNED=$(mktemp -u /tmp/oxdesk-assinado-XXXX.exe)
step "windows: sign"
# ALWAYS before copying: Defender removed the raw .exe the instant it was
# written, twice, taking the scheduled task with it. See specs/008.
if out=$(cp "$EXE" "$SIGNED" && bash scripts/sign-windows.sh "$SIGNED" 2>&1); then ok; else fail "$out"; fi

step "windows: upload and swap"
if out=$(send "$SIGNED" oxdesk); then
  out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'oxdesk'
$new = Join-Path $dst 'oxdesk.new.exe'
if (-not (Test-Path $new)) { Write-Output 'ERROR: the file did not arrive (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $new
if ($v.Status -ne 'Valid') { Write-Output ("ERROR: signature " + $v.Status); exit 1 }
Get-Process -Name oxdesk -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 600
# Size BEFORE moving, compared AFTER. In PowerShell, not passed through
# bash: a valid signature does not guarantee the file stayed (Defender
# silently reverted the agent's write for an hour while this script said
# "ok"), and bringing the number in from outside would need an expanding
# heredoc -- which is how I broke this very step.
$expected = (Get-Item $new).Length
Copy-Item -Force $new (Join-Path $dst 'oxdesk.exe')
Remove-Item -Force $new
Start-Sleep -Milliseconds 500
$got = (Get-Item (Join-Path $dst 'oxdesk.exe')).Length
if ($got -ne $expected) {
  Write-Output ('ERROR: written but has ' + $got + ' bytes, expected ' + $expected + ' -- antivirus reverting? check the exclusion for ' + $dst)
  exit 1
}
schtasks /run /tn 'oxdesk' | Out-Null
exit 0
PS1
)
  if bad "$out"; then fail "$out"; else ok; fi
else fail "$out"; fi
rm -f "$SIGNED"

step "windows: GUI shell"
# Only `main.js`: the rest of the shell is `node_modules` (299 MB of
# Chromium) and does not change. The file started to matter because it is
# where the build watcher lives -- the page also knows how to reload
# itself, but that code TRAVELS WITH THE PAGE, and a window opened before
# the feature existed never receives it. That is what left the user
# looking at yesterday's panel.
#
# Takes effect on the app's next launch; we do not kill anyone's window.
if out=$(timeout 60 scp -q gui/main.js \
           "$WIN:AppData/Local/oxdesk/gui/main.js" 2>&1); then ok
else fail "$out"; fi

step "windows: agent"
# The agent WAS LEFT BEHIND for a whole day: this script updated
# `oxdesk.exe` and forgot `oxdesk-agent.exe`, which is exactly what Ubuntu
# invokes over SSH to read devices and request a channel switch. An old
# binary on the other side of the bridge is the kind of divergence that
# raises no error -- it answers, it just answers yesterday's answer.
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path agent/Cargo.toml 2>&1); then
  AG=agent/target/x86_64-pc-windows-gnu/release/oxdesk-agent.exe
  AGSIG=$(mktemp -u /tmp/oxdesk-agent-assinado-XXXX.exe)
  if out=$(cp "$AG" "$AGSIG" && bash scripts/sign-windows.sh "$AGSIG" 2>&1 &&
           send "$AGSIG" oxdesk-agent); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'oxdesk'
$new = Join-Path $dst 'oxdesk-agent.new.exe'
$target = Join-Path $dst 'oxdesk-agent.exe'
if (-not (Test-Path $new)) { Write-Output 'ERROR: the file did not arrive (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $new
if ($v.Status -ne 'Valid') { Write-Output ('ERROR: signature ' + $v.Status); exit 1 }
# The agent is invoked and exits; there is no resident process to stop.
# Size BEFORE moving, compared AFTER. In PowerShell, not passed through
# bash: a valid signature does not guarantee the file stayed (Defender
# silently reverted the agent's write for an hour while this script said
# "ok"), and bringing the number in from outside would need an expanding
# heredoc -- which is how I broke this very step.
$expected = (Get-Item $new).Length
Copy-Item -Force $new $target
Remove-Item -Force $new
Start-Sleep -Milliseconds 500
$got = (Get-Item $target).Length
if ($got -ne $expected) {
  Write-Output ('ERROR: written but has ' + $got + ' bytes, expected ' + $expected + ' -- antivirus reverting? check the exclusion for ' + $dst)
  exit 1
}
exit 0
PS1
)
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi
  rm -f "$AGSIG"
else fail "$out"; fi

step "windows: updater (oxdesk-ota)"
# Signed like every .exe that lands here: Defender deletes the raw one on
# write (specs/008). Here it does NOT swap binaries -- the Authenticode
# key never leaves Ubuntu, so on that side oxdesk-ota checks and reports,
# and this script is what swaps.
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path ota/Cargo.toml 2>&1); then
  OT=ota/target/x86_64-pc-windows-gnu/release/oxdesk-ota.exe
  OTSIG=$(mktemp -u /tmp/oxdesk-ota-assinado-XXXX.exe)
  if out=$(cp "$OT" "$OTSIG" && bash scripts/sign-windows.sh "$OTSIG" 2>&1 &&
           send "$OTSIG" oxdesk-ota); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'oxdesk'
$new = Join-Path $dst 'oxdesk-ota.new.exe'
$target = Join-Path $dst 'oxdesk-ota.exe'
if (-not (Test-Path $new)) { Write-Output 'ERROR: the file did not arrive (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $new
if ($v.Status -ne 'Valid') { Write-Output ('ERROR: signature ' + $v.Status); exit 1 }
$expected = (Get-Item $new).Length
Copy-Item -Force $new $target
Remove-Item -Force $new
Start-Sleep -Milliseconds 500
$got = (Get-Item $target).Length
if ($got -ne $expected) {
  Write-Output ('ERROR: written but has ' + $got + ' bytes, expected ' + $expected + ' -- antivirus reverting? check the exclusion for ' + $dst)
  exit 1
}
exit 0
PS1
)
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi
  rm -f "$OTSIG"
else fail "$out"; fi

step "windows: screen viewer"
# The viewer stopped being Mac-only when NVDEC started decoding -- and it
# is what removes the last RDP from the system. No `libavcodec` on that
# side: the .exe's DLL list has only Windows stuff, and the decoder comes
# with the NVIDIA driver, loaded at runtime.
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path stream/Cargo.toml --bin oxdesk-view 2>&1); then
  VW=stream/target/x86_64-pc-windows-gnu/release/oxdesk-view.exe
  VWSIG=$(mktemp -u /tmp/oxdesk-view-assinado-XXXX.exe)
  if out=$(cp "$VW" "$VWSIG" && bash scripts/sign-windows.sh "$VWSIG" 2>&1 &&
           send "$VWSIG" oxdesk-view); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'oxdesk'
$new = Join-Path $dst 'oxdesk-view.new.exe'
if (-not (Test-Path $new)) { Write-Output 'ERROR: the file did not arrive (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $new
if ($v.Status -ne 'Valid') { Write-Output ('ERROR: signature ' + $v.Status); exit 1 }
# It may be open: whoever is watching closes the window when they want.
Get-Process -Name oxdesk-view -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
# Size BEFORE moving, compared AFTER. In PowerShell, not passed through
# bash: a valid signature does not guarantee the file stayed (Defender
# silently reverted the agent's write for an hour while this script said
# "ok"), and bringing the number in from outside would need an expanding
# heredoc -- which is how I broke this very step.
$expected = (Get-Item $new).Length
Copy-Item -Force $new (Join-Path $dst 'oxdesk-view.exe')
Remove-Item -Force $new
Start-Sleep -Milliseconds 500
$got = (Get-Item (Join-Path $dst 'oxdesk-view.exe')).Length
if ($got -ne $expected) {
  Write-Output ('ERROR: written but has ' + $got + ' bytes, expected ' + $expected + ' -- antivirus reverting? check the exclusion for ' + $dst)
  exit 1
}
exit 0
PS1
)
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi
  rm -f "$VWSIG"
else fail "$out"; fi

step "windows: tab bridge"
# Without it, Connect on Windows falls back to the native window (specs/016).
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path stream/Cargo.toml --bin oxdesk-bridge 2>&1); then
  PT=stream/target/x86_64-pc-windows-gnu/release/oxdesk-bridge.exe
  PTSIG=$(mktemp -u /tmp/oxdesk-bridge-assinado-XXXX.exe)
  if out=$(cp "$PT" "$PTSIG" && bash scripts/sign-windows.sh "$PTSIG" 2>&1 &&
           send "$PTSIG" oxdesk-bridge); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'oxdesk'
$new = Join-Path $dst 'oxdesk-bridge.new.exe'
if (-not (Test-Path $new)) { Write-Output 'ERROR: the file did not arrive (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $new
if ($v.Status -ne 'Valid') { Write-Output ('ERROR: signature ' + $v.Status); exit 1 }
# An open tab holds the file: the session drops and reopens.
Get-Process -Name oxdesk-bridge -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
# Size BEFORE moving, compared AFTER. In PowerShell, not passed through
# bash: a valid signature does not guarantee the file stayed (Defender
# silently reverted the agent's write for an hour while this script said
# "ok"), and bringing the number in from outside would need an expanding
# heredoc -- which is how I broke this very step.
$expected = (Get-Item $new).Length
Copy-Item -Force $new (Join-Path $dst 'oxdesk-bridge.exe')
Remove-Item -Force $new
Start-Sleep -Milliseconds 500
$got = (Get-Item (Join-Path $dst 'oxdesk-bridge.exe')).Length
if ($got -ne $expected) {
  Write-Output ('ERROR: written but has ' + $got + ' bytes, expected ' + $expected + ' -- antivirus reverting? check the exclusion for ' + $dst)
  exit 1
}
exit 0
PS1
)
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi
  rm -f "$PTSIG"
else fail "$out"; fi

step "windows: screen host (oxdesk-stream)"
# Windows became a HOST of our protocol (v4): DXGI -> NVENC -> QUIC.
# It needs a signature for the same reason as oxdesk -- Defender deleted
# the raw .exe the instant it was written, twice (specs/008).
#
# And it needs a SCHEDULED TASK, not a launch over SSH: SSH lands in
# session 0, where Desktop Duplication does not exist and SendInput
# reaches no desktop at all. `/it` makes the task run in the interactive
# session.
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path stream/Cargo.toml --bin oxdesk-host 2>&1); then
  HOSTEXE=stream/target/x86_64-pc-windows-gnu/release/oxdesk-host.exe
  HOSTSIG=$(mktemp -u /tmp/oxdesk-host-assinado-XXXX.exe)
  # Delete the destination BEFORE the scp.
  #
  # The Windows SFTP server refuses to overwrite with "Overwriting an
  # existing file is not supported" -- and a deploy interrupted midway
  # (that was the case: I killed the signing that hung on the TSA) leaves
  # the `.new.exe` behind and poisons every later deploy. A step that only
  # works if the previous one finished cleanly is not a step, it is a
  # trap.
  if out=$(cp "$HOSTEXE" "$HOSTSIG" && bash scripts/sign-windows.sh "$HOSTSIG" 2>&1 &&
           send "$HOSTSIG" oxdesk-host); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'oxdesk'
$new = Join-Path $dst 'oxdesk-host.new.exe'
$target = Join-Path $dst 'oxdesk-host.exe'
if (-not (Test-Path $new)) { Write-Output 'ERROR: the file did not arrive (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $new
if ($v.Status -ne 'Valid') { Write-Output ('ERROR: signature ' + $v.Status); exit 1 }
Get-Process -Name oxdesk-host -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
# Size BEFORE moving, compared AFTER. In PowerShell, not passed through
# bash: a valid signature does not guarantee the file stayed (Defender
# silently reverted the agent's write for an hour while this script said
# "ok"), and bringing the number in from outside would need an expanding
# heredoc -- which is how I broke this very step.
$expected = (Get-Item $new).Length
Copy-Item -Force $new $target
Remove-Item -Force $new
Start-Sleep -Milliseconds 500
$got = (Get-Item $target).Length
if ($got -ne $expected) {
  Write-Output ('ERROR: written but has ' + $got + ' bytes, expected ' + $expected + ' -- antivirus reverting? check the exclusion for ' + $dst)
  exit 1
}
# The task runs the .exe DIRECTLY, with no .cmd wrapper.
#
# There used to be a .cmd here just to redirect output, because schtasks
# is not a shell. The price was a visible console window in the
# interactive session -- right in the middle of the screen someone is
# watching. The user reported it as "that terminal that opens when
# connecting to the screen".
#
# Now the host is `windows_subsystem = "windows"` (as oxdesk already was)
# and redirects its own output to the same log file before the first
# line. No cmd.exe in the path, no window, same log.
# No argument: the host defaults to 9777. Passing the number here would
# require nesting quotes in /tr, which has already broken three times on
# this bridge.
$env1 = $target
Remove-Item (Join-Path $dst 'oxdesk-host.cmd') -ErrorAction SilentlyContinue
# A task that does NOT run on its own: `once` at 00:00 has already
# passed, and we only wake it with `schtasks /run`. `/it` is the point --
# without it the capture lands in session 0, where there is no desktop to
# duplicate and no desktop for SendInput to reach.
$r = schtasks /create /tn oxdesk-host /tr $env1 /sc once /st 00:00 /it /f 2>&1
# The exit code is 0 with a WARNING about /ST being in the past: that is
# expected and wanted. A real error comes with a code != 0.
if ($LASTEXITCODE -ne 0) { Write-Output ('ERROR: schtasks ' + $r); exit 1 }
exit 0
PS1
)
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi
  rm -f "$HOSTSIG"
else fail "$out"; fi

# ---------- mac ----------
if [ "$SKIP_MAC" -eq 1 ]; then
  echo "  mac: skipped"
else
  step "mac: send source"
  # `oxdesk-stream` goes along: the Mac is the remote-screen CLIENT, and
  # leaving its binary behind has already happened once -- the one who
  # noticed was the user, not me.
  #
  # `ota` too: the Mac is exactly where OTA has a reason to exist, and
  # sending the rest without it would leave the machine that travels
  # without the only path it has to update itself away from home
  # (specs/015).
  if out=$(for d in hidpp oxdesk stream ota; do
             rsync -az --delete --exclude target -e "ssh -o BatchMode=yes" "$d" "$MAC:~/oxdesk/" || exit 1
           done 2>&1); then ok; else fail "$out"; fi

  step "mac: updater (oxdesk-ota)"
  # Built THERE, like every Mac binary. It deliberately does not depend on
  # brew (build-binaries.sh refuses if it does): whatever repairs the
  # machine must not need ffmpeg to come up.
  if out=$(timeout 300 ssh -n -o BatchMode=yes "$MAC" \
            "cd ~/oxdesk/ota && OXDESK_BUILD='$BUILD' OXDESK_BUILD_N='$OXDESK_BUILD_N' ~/.cargo/bin/cargo build --release 2>&1 | tail -3 &&
             install -m755 target/release/oxdesk-ota ~/.local/bin/oxdesk-ota &&
             ~/.local/bin/oxdesk-ota --help > /dev/null" 2>&1); then ok; else fail "$out"; fi
  step "mac: build and restart"
  # No cross-compile from Linux to macOS: it needs Apple's SDK, so the
  # build happens there. OXDESK_BUILD travels along because the copy has no
  # .git. Two things this step learned on a day it said `ok` seven times
  # in a row about a Mac serving a ten-day-old binary:
  #
  # 1. `pkill -x oxdesk` before the kickstart. `-k` only kills launchd's
  #    copy; the GUI's orphan child keeps the port and the new daemon
  #    dies with `Address already in use`.
  # 2. Check `/healthz`. Asking for a restart is not having restarted --
  #    see .claude/rules/verify-what-arrived.md.
  if out=$(timeout 420 ssh -n -o BatchMode=yes "$MAC" \
            "cd ~/oxdesk/oxdesk && OXDESK_BUILD='$BUILD' OXDESK_BUILD_N='$OXDESK_BUILD_N' ~/.cargo/bin/cargo build --release 2>&1 | tail -3 &&
             install -m755 target/release/oxdesk ~/.local/bin/oxdesk || exit 1
             pkill -x oxdesk
             launchctl kickstart -k gui/\$(id -u)/app.oxdesk.daemon
             for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
               sleep 2
               curl -s --max-time 2 localhost:8787/healthz | grep -q '$BUILD' && exit 0
             done
             echo \"ERROR: Mac /healthz did not confirm $BUILD: \$(curl -s --max-time 2 localhost:8787/healthz)\"
             exit 1" 2>&1); then
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi

  step "mac: screen (oxdesk-stream, client and host)"
  # brew's PKG_CONFIG_PATH: `ffmpeg-next` only finds libavcodec through
  # it, and SSH does not load the shell profile.
  #
  # `ffmpeg@7` and not `ffmpeg`: the binary CI builds will replace THIS
  # one through OTA, and both must reference the same prefix. See
  # FFMPEG_FORMULA in build-binaries.sh.
  #
  # The HOST was missing here. Only the client was built, so the machine
  # could watch the others and never be watched -- and the panel fell
  # back to VNC for lack of an installed alternative. The symptom arrived
  # as "I tested but it called vnc, shouldn't it be native?".
  #
  # The check below does NOT require the `ffmpeg@7` prefix, and that is
  # deliberate. On this Mac `ffmpeg@7` is not a keg: it is an ALIAS
  # (`/opt/homebrew/opt/ffmpeg@7 -> ../Cellar/ffmpeg/7.1_4`), and the
  # `libavcodec.pc` inside it says `prefix=.../Cellar/ffmpeg/7.1_4`.
  # That is: pkg-config there answers the UNVERSIONED prefix no matter
  # how much PKG_CONFIG_PATH you set, and the binary built here will
  # always say `/opt/homebrew/opt/ffmpeg/...`.
  #
  # Coexisting with the CI package (which links `opt/ffmpeg@7`, where the
  # runner has the real keg) works because BOTH paths exist on this
  # machine and point to the same keg -- measured: the host that came
  # from OTA captured. What needs to be true is weaker and more useful:
  # the dylibs resolve, and the major is the declared one.
  #
  # See FFMPEG_SONAME in build-binaries.sh, which on the PACKAGE path
  # is strict on purpose -- a package linked to the unversioned prefix
  # does not install on a machine that only has the versioned keg.
  if out=$(timeout 900 ssh -n -o BatchMode=yes "$MAC" \
            "cd ~/oxdesk/stream && export PATH=/opt/homebrew/bin:\$PATH PKG_CONFIG_PATH=/opt/homebrew/opt/ffmpeg@7/lib/pkgconfig &&
             ~/.cargo/bin/cargo build --release --bin oxdesk-view --bin oxdesk-bridge --bin oxdesk-host 2>&1 | tail -3 &&
             mkdir -p ~/.local/bin ~/.local/state &&
             install -m755 target/release/oxdesk-view ~/.local/bin/ &&
             install -m755 target/release/oxdesk-bridge ~/.local/bin/ &&
             install -m755 target/release/oxdesk-host ~/.local/bin/ || exit 1
             for b in oxdesk-host oxdesk-view; do
               otool -L ~/.local/bin/\$b | grep -q 'libavcodec.61.dylib' ||
                 echo \"ERROR: \$b does not ask for libavcodec.61 -- did the ffmpeg major change? see FFMPEG_SONAME\"
               for d in \$(otool -L ~/.local/bin/\$b | grep -o '/opt/[^ ]*'); do
                 [ -f \"\$d\" ] || echo \"ERROR: \$b asks for \$d and it does not exist on this machine\"
               done
             done" 2>&1); then
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi

  step "mac: sign the host"
  # A stable signature is what makes the Screen Recording permission
  # survive a rebuild: an ad-hoc binary ties TCC to the cdhash, which
  # always changes.
  #
  # Runs directly over SSH because the identity lives in a keychain of
  # OUR OWN, with a known password and an adjusted partition list. In the
  # login keychain this was impossible: codesign asked for a GUI dialog on
  # every signature, and over SSH it failed with `errSecInternalComponent`.
  #
  # FAILS if the binary stays ad-hoc. The previous version of this step
  # had a `|| true` and said ok with the signature not applied -- and I
  # only found out because the Mac's screen started freezing again.
  if out=$(ssh -n -o BatchMode=yes "$MAC" \
            "~/.local/bin/sign.sh ~/.local/bin/oxdesk-host 2>&1 | tail -3;
             if codesign -dv ~/.local/bin/oxdesk-host 2>&1 | grep -q adhoc; then
               echo 'ERROR: the binary stayed ad-hoc; run create-identity.sh on the Mac'
               exit 1
             fi" 2>&1); then ok; else fail "$out"; fi

  step "mac: signing scripts"
  # Signing the host with a STABLE identity is what makes the Screen
  # Recording permission survive a rebuild: cargo's binary is
  # ad-hoc/linker-signed and the cdhash always changes, so TCC loses the
  # grant on every deploy. The identity itself is created by hand, once,
  # in the Mac's Terminal (`create-identity.sh`) -- over SSH the keychain
  # is locked.
  if out=$(scp -q -o BatchMode=yes mac/create-identity.sh mac/sign.sh "$MAC:.local/bin/" 2>&1 &&
           ssh -n -o BatchMode=yes "$MAC" "chmod +x ~/.local/bin/create-identity.sh ~/.local/bin/sign.sh" 2>&1); then ok; else fail "$out"; fi

  step "mac: screen host LaunchAgent"
  # The host MUST be born in the graphical session: capture depends on a
  # TCC permission granted to whoever launched it, and over SSH
  # AVFoundation hangs instead of failing. `RunAtLoad false` -- it only
  # comes up when someone asks for the screen, so it does not burn battery
  # capturing for nobody.
  if out=$(scp -q -o BatchMode=yes mac/app.oxdesk.host.plist "$MAC:Library/LaunchAgents/" 2>&1 &&
           ssh -n -o BatchMode=yes "$MAC" \
            "launchctl bootout gui/\$(id -u)/app.oxdesk.host 2>/dev/null || true;
             # bootstrap fails with an I/O error if the previous one is
             # still exiting. A short pause covers it, and retrying is
             # idempotent.
             sleep 1
             launchctl bootstrap gui/\$(id -u) ~/Library/LaunchAgents/app.oxdesk.host.plist 2>/dev/null || \
             launchctl bootstrap gui/\$(id -u) ~/Library/LaunchAgents/app.oxdesk.host.plist" 2>&1); then ok; else fail "$out"; fi

  step "mac: GUI shell"
  # Windows got a loose `main.js` on every deploy; the Mac had the shell
  # inside an `app.asar`, editable only with an asar tool -- which exists
  # on neither this machine nor that one. Result: the Mac's shell was
  # weeks old and NOBODY knew, because no step touched it. That is how the
  # "one supervisor per daemon" fix almost did not reach the machine that
  # needed it most.
  #
  # Converted to `Resources/app/`, Electron's classic layout, which is
  # still supported: from here on the Mac's shell travels by rsync like
  # the Windows one. `app.asar` becomes `.off` instead of disappearing,
  # `ElectronAsarIntegrity` leaves Info.plist (it stamps the hash of an
  # asar that is no longer read) and the bundle is re-signed ad-hoc so the
  # seal is not broken. All idempotent.
  #
  # We do not restart the GUI here: it reads `main.js` on its next launch,
  # and killing the window of whoever is using the machine is not the
  # deploy's job.
  if out=$(ssh -n -o BatchMode=yes "$MAC" \
             'mkdir -p ~/Applications/Oxdesk.app/Contents/Resources/app' 2>&1 &&
           rsync -az -e "ssh -o BatchMode=yes" gui/main.js gui/package.json gui/icon.png gui/tray.png \
             "$MAC:Applications/Oxdesk.app/Contents/Resources/app/" 2>&1 &&
           ssh -n -o BatchMode=yes "$MAC" '
             APP=$HOME/Applications/Oxdesk.app
             if [ -f "$APP/Contents/Resources/app.asar" ]; then
               mv "$APP/Contents/Resources/app.asar" "$APP/Contents/Resources/app.asar.off"
               /usr/libexec/PlistBuddy -c "Delete :ElectronAsarIntegrity" "$APP/Contents/Info.plist" >/dev/null 2>&1
               codesign --force --deep --sign - "$APP" 2>&1 | tail -1
             fi
             grep -q supervisorCommand "$APP/Contents/Resources/app/main.js" ||
               echo "ERROR: the main.js that reached the Mac is not the one from this repo"' 2>&1); then
    if bad "$out"; then fail "$out"; else ok; fi
  else fail "$out"; fi
fi

# ---------- OTA token ----------
# The Releases read token is a fine-grained PAT with `Contents: read` ONLY
# on this repo. It lives here and is distributed; without it the API
# answers 200 with {"message":"Not Found"} and the wizard says exactly
# that. See specs/015.
TOKEN="$HOME/.config/oxdesk/ota-token"
if [ -f "$TOKEN" ]; then
  step "windows: OTA token"
  if out=$(timeout 60 scp -q "$TOKEN" \
             "$WIN:AppData/Local/oxdesk/ota-token" 2>&1); then ok
  else fail "$out"; fi
  if [ "$SKIP_MAC" -eq 0 ]; then
    step "mac: OTA token"
    # `chmod 600` on that side: scp preserves the source mode, but relying
    # on that is relying on another machine's umask -- and the wizard
    # refuses a token readable by others.
    if out=$(ssh -n -o BatchMode=yes "$MAC" 'mkdir -p ~/.config/oxdesk' 2>&1 &&
             scp -q -o BatchMode=yes "$TOKEN" "$MAC:.config/oxdesk/ota-token" 2>&1 &&
             ssh -n -o BatchMode=yes "$MAC" 'chmod 600 ~/.config/oxdesk/ota-token' 2>&1); then ok
    else fail "$out"; fi
  fi
else
  echo "  OTA token: does not exist at $TOKEN -- OTA will not work (specs/015)"
fi

# ---------- final check ----------
# The lines printed from here on are the ones a person reads to decide
# the fleet is consistent, and they are quoted verbatim in
# .claude/skills/deploy.md. `== check ==`, `OUT OF SYNC` and `all three
# machines on the same build` move together with that file or not at all
# -- see .claude/rules/english-only.md. Nothing machine-parses them: the
# only reader is that skill (checked across the repo and .github).
echo "== check =="
sleep 6
declare -A seen
seen[ubuntu]=$(curl -s --max-time 8 http://127.0.0.1:8787/healthz 2>/dev/null)
seen[windows]=$(echo "(Invoke-WebRequest -UseBasicParsing -TimeoutSec 8 'http://127.0.0.1:8787/healthz').Content" | psrun)
[ "$SKIP_MAC" -eq 1 ] || seen[mac]=$(timeout 25 ssh -n -o BatchMode=yes "$MAC" \
  'curl -s --max-time 8 http://127.0.0.1:8787/healthz' 2>/dev/null)

for h in "${!seen[@]}"; do
  b=$(printf '%s' "${seen[$h]}" | sed -n 's/.*"build":"\([^"]*\)".*/\1/p')
  n=$(printf '%s' "${seen[$h]}" | sed -n 's/.*"n":\([0-9]*\).*/\1/p')
  if [ "$b" = "$BUILD" ]; then
    printf '  %-10s #%s %s\n' "$h" "${n:-?}" "$b"
  else
    printf '  %-10s %s  <-- OUT OF SYNC (expected %s)\n' "$h" "${b:-no answer}" "$BUILD"
    failures=$((failures+1))
  fi
done

echo
if [ "$failures" -ne 0 ]; then
  echo "$failures problem(s)"
elif [ "$SKIP_MAC" -eq 1 ]; then
  # NOT "all three machines": with --skip-mac the mac was never asked, and
  # saying three when two were measured is the same silence the rules are
  # about. This is the line a person reads to decide the fleet is
  # consistent, so it has to name what was left out.
  echo "${#seen[@]} of 3 machines on the same build: $BUILD (mac left out: --skip-mac)"
else
  echo "all three machines on the same build: $BUILD"
fi
exit "$failures"
