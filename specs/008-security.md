# 008 — Security and trust model

> **Changed on 2026-11-07:** [`017`](017-product.md) replaces "no telemetry"
> with minimal telemetry that can be turned off, and trust in the LAN with each
> machine's key. The rest of this spec still holds.

Everything on the LAN. No account, no cloud, no telemetry.
Audited in the binaries we already use (see README): Deskflow, Solaar and
GSConnect have no telemetry endpoint; the only one able to talk to the
internet is Deskflow's opt-in update check, which ships with
`enableUpdateCheck=false`.

## Why localhost only

`logid` listens on `127.0.0.1`, not on `0.0.0.0`. It **reads ports,
runs SSH and triggers HID++ channel switches** — it is privilege
surface, not a website. Whoever reaches the API reaches both machines.

If one day you want to open it to the phone, do not bind on `0.0.0.0`: expose it
through an SSH tunnel or a reverse proxy with auth. The difference is
intentional.

## Trust between the machines

Three credential pairs, each belonging to the owning tool. **Do not
centralize** — reinventing key management is how keys get lost.

| Channel | Credential | Verification |
|---|---|---|
| SSH | ed25519 **with passphrase**, in `gcr-ssh-agent` | fingerprint in `known_hosts`, checked when migrating IP→name |
| Deskflow | TLS, cert in `~/.config/Deskflow/tls/` | fingerprint approved on first contact (`trusted-servers`) |
| KDE Connect | mutual TLS, cert per device | *verification key* `4BCF0AF8` |

All three use **trust on first use with human confirmation**. This is not
friction to eliminate: it is the only defense against someone on the same LAN posing
as the other machine. A "one-click pairing" that accepts a fingerprint
on its own removes the whole protection.

**Rule:** `logid` may *show* the fingerprints side by side to
make comparison easier. It may never accept on your behalf.

## Risks enabled today, consciously

| Risk | State | Mitigation |
|---|---|---|
| GNOME RDP on 3389, password, whole LAN | **on** | `grdctl rdp disable`; the panel should show this as a risk |
| Clipboard mirroring the Bitwarden password | mitigated | `send-content=false` — one-way ([`004`](004-clipboard.md)) |
| SSH key with admin access on Windows | accepted | passphrase + session agent |
| `hidraw` accessible to the user | necessary | `uaccess` via udev, only for the locally logged-in user |
| Input traveling over the LAN (Deskflow) | accepted | TLS 1.3, fingerprint checked |

## What never to store

- Passphrase, password or private key in `config.toml` or in the state.
- Clipboard content in a log.
- A fingerprint accepted automatically, without a record of when and by whom.

## Failure mode that is a security problem

Losing keyboard and mouse on both machines (see
[`005`](005-input-switch.md#there-is-no-rollback-on-this-side)) leaves you with no way
to intervene on an unlocked machine. Automatic rollback is not a convenience:
it is access control.

---

## Defender deleted the daemon, and was being reasonable (2026-11-04)

`Trojan:Win32/Bearfoos.A!ml` — **quarantine and removal** of
`logid.exe`, plus deletion of the scheduled task and the corresponding
`TaskCache` keys. In the middle of a test session, which incidentally
explained the "it hangs and won't budge no matter what": there was no daemon.

### Why it took issue

The **`!ml`** suffix is model-based detection, not a signature — no
known malware matched. What matched was the **profile**, and fairly so:

| what the daemon does | how a model reads it |
|---|---|
| new executable, unsigned, no reputation | freshly planted binary |
| creates a scheduled task that runs at login | persistence |
| runs `ssh`, `taskkill`, `tasklist`, `sc` | remote command execution |
| reads an HID device in raw mode | input capture |
| starts a local HTTP server | control channel |

Each item was asked for. Together, they draw the silhouette of a RAT. The
detection listed the **file and the task together** — it was the combination.

### The chosen way out: sign, not exclude

The user preferred signing to opening an exception in the antivirus. An exclusion
would solve it in one command, but it turns off inspection of a path — and
inspection is exactly what protects against someone writing there.

**Signing on Windows did not work**: real-time protection removed the
`.exe` the instant it was written, twice, before there was a chance to
sign it. It is a race that cannot be won on that side.

So the binary now arrives **already signed**, via
[`scripts/sign-windows.sh`](../scripts/sign-windows.sh):

- key pair generated **here** with openssl (`~/.config/logi/signing`, `600`, outside
  the repo). The private key never leaves this machine — better than exporting
  the private key of a certificate created on Windows.
- `osslsigncode` (installed via linuxbrew, no sudo) signs with SHA-256 and a
  DigiCert timestamp, so the signature survives the
  certificate's expiry.
- only the **public** certificate goes to Windows, imported into
  `LocalMachine\Root` and `LocalMachine\TrustedPublisher`. It is valid on that
  machine and no other.

Result checked by Windows itself:

```
assinatura: [Valid] Assinatura verificada.
assinante:  O=infra pessoal, CN=logi (assinatura local)
carimbo:    CN=DigiCert SHA256 RSA4096 Timestamp Responder 2026 1
```

The file survived the write, the task was recreated and the daemon came up.

> **Every Windows build goes through the script.** Sending a raw `.exe`
> there is not just risky — it has already been deleted twice, and the second time
> took the scheduled task with it.

### If it happens again

A self-made signature guarantees nothing against behavioral heuristics:
the model weighs what the program does, and what it does has not changed. If
it comes back, the ways out are, in order of least damage:

1. exclusion of the **exact file path** (not the folder) plus the
   process — scope of one file;
2. submission to Microsoft as a false positive (takes days, but fixes it for
   everyone);
3. folder exclusion — widest scope, last resort.

## Defender exclusion, and why it was necessary (2026-11-04)

Defender acted three times on our binaries — its history
lists `logid.exe` and `logid.new.exe` as **`Trojan:Win32/Bearfoos.A!ml`**.
The `!ml` suffix is a model's guess, not a known-malware signature;
the silhouette (new executable, no reputation, that creates a scheduled task,
runs remote commands and reads an input device in raw mode) is
genuinely similar.

Signing solved it for `logid.exe`. **It did not solve it for
`logid-agent.exe`**: the same signed bytes were accepted under another
name and silently reverted under the right name. Windows spent an hour
running the previous version while the deploy said `ok`.

The user authorized the exclusion of `%LOCALAPPDATA%\logid`. The cost is
real and is written down here: **Defender stopped scanning that folder**,
so whatever lands there is not checked. In exchange, delivery stopped
depending on a heuristic that changes its mind between signature versions.

### The lesson worth more than the exclusion

The deploy step said **"ok" without checking the result**. `Move-Item`
did not complain, the antivirus reverted afterwards, and the report lied. Now
the expected size travels in the command and the step compares it after writing.

And the check was tested with a deliberately wrong size, until it
failed:

```
ERRO: gravou mas o arquivo tem 458792 bytes, esperado 999999
      -- antivirus revertendo? confira a exclusao de C:\...\logid
```

A check that has never failed is not a known check. This applies to the
other deploy stages that today only look at the signature.
