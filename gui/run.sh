#!/usr/bin/env bash
# Starts the desktop app. Clearing ELECTRON_RUN_AS_NODE is mandatory when
# the terminal comes from inside VS Code (the extension host exports it,
# and with it the Electron binary runs as plain Node and nothing opens).
cd "$(dirname "$0")"
exec env -u ELECTRON_RUN_AS_NODE ./node_modules/.bin/electron . "$@"
