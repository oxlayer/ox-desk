# Enable and operate automatic updates (OTA)

What makes the machine download the published build and swap itself,
without anyone running the deploy. Design and rationale:
[`specs/015`](../../specs/015-ota.md).

## 1. The token (once, and it's the only manual part)

The repo is private, so without a token the API answers `200` with
`{"message":"Not Found"}` and nothing works.

Create a **fine-grained PAT** at
<https://github.com/settings/personal-access-tokens/new>:

| Field | Value |
|---|---|
| Resource owner | `oxlayer` |
| Repository access | Only select repositories → `ox-desk` |
| Permissions → Repository | **Contents: Read-only** |
| Expiration | whatever you are willing to renew |

Nothing beyond `Contents: Read-only`. Do **not** use the `gh` token: it
has `repo` and `workflow`, i.e. write access to everything — and this
file goes to a laptop that leaves the house.

```bash
install -m600 /dev/null ~/.config/oxdesk/ota-token
printf '%s' 'github_pat_...' > ~/.config/oxdesk/ota-token
```

`deploy.sh` distributes it to Windows and the Mac from here (and does
`chmod 600` on the other side). The wizard **refuses** a token readable
by others.

## 2. Turn on the clock on each machine

In the wizard (`/setup` in the panel, or the app opens on it by itself
when something essential is missing), the **Atualização automática
(OTA)** (Automatic update) step has a button. It installs the system
scheduler — systemd timer, LaunchAgent, scheduled task — and does a
first check in `--conferir` mode.

## 3. Publish a version for the machines to consume

```bash
git tag v0.3.0 && git push origin v0.3.0
```

Without a tag there is no Release, and without a Release the OTA has
nothing to download — it says `igual` (same) forever. Recipe and cost in
[`specs/013`](../../specs/013-ci-cd.md#publishing-a-version).

## Asking by hand

```bash
oxdesk-ota --conferir   # only looks, changes nothing
oxdesk-ota --estado     # the JSON of the last check
oxdesk-ota              # checks and applies, if there is something new
oxdesk-ota --forcar     # reinstalls the published one even if it's the same build
```

On **Windows** none of them swaps the binary: the Authenticode key
doesn't leave the Ubuntu machine, so there the OTA only checks and
reports. That's expected, and the panel shows "há o build #N publicado"
(build #N is published) as a permanent pending item until someone runs
the deploy.

## When it goes wrong

| Symptom | What it is |
|---|---|
| `o GitHub respondeu: Not Found` | the token is missing, or it doesn't grant `Contents: read` on this repo |
| `o GitHub respondeu: Bad credentials` | invalid or expired token |
| `nenhum pacote comecando com oxdesk-mac-...` | the Release went out without that package (Mac job without a runner?) |
| `o pacote nao confere` | corrupted download; the next round tries again |
| `revertido` | the new build didn't answer `/healthz`; the previous one came back. The copy stays in `~/.local/state/oxdesk-ota/anterior/` |
| `o relógio não está acordando` | the last check was more than 6 h ago — the scheduler went down |

After a boot, the Mac may take **up to an hour** to look: the
LaunchAgent is `RunAtLoad false` on purpose, because installing it must
not apply an update in the middle of the request that installed it.

## See also

- [[deploy]] — the in-house path, which signs and swaps everything
- [[configure-a-machine]] — the wizard where the step lives
