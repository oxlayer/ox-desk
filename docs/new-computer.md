# New computer

The goal is not to depend on memory: install `oxdesk`, open the wizard at
`http://127.0.0.1:8787/setup` and press the buttons in the order it
shows. The installers ship inside the binary, so the repo isn't needed.

## Mac

1. Install [Homebrew](https://brew.sh) and `oxdesk`.
2. In the wizard, press the steps in order. The permissions come after
   the **Screen host signed** step.
3. At the **Tailscale** step, Terminal opens: type the password and open
   the login link. **Do not install the app** ([`specs/012`](../specs/012-outside-the-lan.md)).

## Windows

1. Install `oxdesk` and Tailscale, and log in to Tailscale.
2. In the wizard, press the steps in order.
3. At the **A screen with no monitor attached** step, accept the
   administrator prompt.

## Ubuntu

1. `./install.sh` and `sudo bash scripts/install-tailscale.sh`.
2. In the wizard, press the steps in order. With no monitor on, the host
   uses a virtual screen by itself, and there is no step for that.

## What remains manual

- The Tailscale login: it is the consent of the account's owner.
- The macOS permissions: Apple doesn't allow granting them from outside.
- The OTA token, which is created once and distributed with
  `scripts/distribute-ota-token.sh`
  ([skill](../.claude/skills/enable-ota.md)).
