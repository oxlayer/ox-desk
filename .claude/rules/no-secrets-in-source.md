# No credential and no authorized identity lives in source

A public repository turns every literal in the tree into a published
string. Two kinds of literal are forbidden, and the second one is the kind
that nearly shipped.

## 1. No credential, ever — not even a local one

Passwords, tokens, passphrases and keychain words come from a file, an
environment variable or a prompt. "It only unlocks a local keychain" is
not an exemption: that keychain holds the Mac's code-signing identity, and
the password sat in `mac/sign.sh` and `mac/create-identity.sh` as the
literal `logi`.

The same audit found why this is also a correctness rule, not only a
secrecy one. The rename changed the literal in one place and not the
others:

| Where | Value |
|---|---|
| `mac/create-identity.sh` | `KCPASS="logi"` — creates the keychain |
| `mac/sign.sh` | `unlock-keychain -p logi` — agrees |
| `oxdesk/src/setup_mac.rs` | `unlock-keychain -p "oxdesk"` — **disagrees** |

The Rust call is `let _ = run_timed(...)`, so the failure is discarded: it
has been unlocking nothing since the rename and nobody noticed, because
the LaunchAgent runs `sign.sh`, which still matches. **A secret duplicated
in three files is three chances to drift**, and a discarded result is how
the drift stays quiet. One source, read at runtime, cannot disagree with
itself.

## 2. No key that a script will authorize on someone else's machine

`scripts/enable-mac.sh` and `windows-ssh.ps1` carried a literal
`ssh-ed25519 …` public key and installed it into the target machine's
`authorized_keys`. A public key is not a secret and publishing it grants
nobody anything — **running the script does**. Anyone following the README
would have given the maintainer SSH access to their own machine without
being asked.

That is a backdoor by accident, and it is the worst thing the pre-publish
audit found. The key comes from whoever runs the script, and with no key
the script **refuses**:

```sh
# ❌ never
PUB='ssh-ed25519 AAAAC3... someone@somewhere'

# ✅ the person running it decides, and silence is not consent
PUB="${1:-$(cat ~/.ssh/id_ed25519.pub 2>/dev/null)}"
[ -n "$PUB" ] || { echo "usage: $0 '<ssh public key>'"; exit 2; }
```

Defaulting to the local `~/.ssh/id_ed25519.pub` is allowed because that
key belongs to the person at the keyboard. Defaulting to anything shipped
in the repo is not.

## Rotate, do not rewrite

When a credential has already been committed, rotation beats history
surgery: a rotated value makes every published copy worthless, and the
history stays citable — and in this repo the history is documentation
([[../../specs/018-open-source]]). A rewrite is for a secret that
**cannot** be rotated, and then it is the right tool.

## Before any publish

`git grep` over all commits for known shapes (`ghp_`, `github_pat_`,
`BEGIN … PRIVATE KEY`, `AKIA…`, `xox[baprs]-`, `AIza…`) is the floor, not
the ceiling: it only finds secrets that look like secrets. A real scanner
(`gitleaks`, `trufflehog`) runs over the full history, and GitHub's secret
scanning and push protection stay on — both free on public repositories.

## See also

- [[no-personal-data]] — the same audit, the other half
- [[verify-what-arrived]] — why a discarded result is the real defect
- [[../../specs/008-security]] — the signing keys never leave their machines
