# Where deploy.sh sends things. Copy to scripts/targets.sh and fill in.
#
#   cp scripts/targets.example.sh scripts/targets.sh
#
# `scripts/targets.sh` is gitignored on purpose: these are YOUR machines,
# and a script someone runs must not carry someone else's.
# See .claude/rules/no-personal-data.md.

# ssh alias (or user@host) of the Windows machine. Required.
# It must work non-interactively: `ssh -o BatchMode=yes <this> echo ok`.
OXDESK_WIN_SSH="windows-pc"

# ssh alias of the Mac. Leave empty if you have none -- the deploy then
# behaves as if --skip-mac had been passed.
OXDESK_MAC_SSH="the-mac"

# The LAN range allowed to reach ssh, for enable-sshd.sh / enable-ufw.sh.
# Find yours with:  ip -o -f inet addr show | awk '{print $4}'
OXDESK_LAN_CIDR=""
