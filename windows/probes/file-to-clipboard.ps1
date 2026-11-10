# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# Copies a file to the clipboard of the INTERACTIVE SESSION.
# It exists to test the Windows -> peers direction without someone clicking
# in Explorer. Over SSH it does not work: that is session 0.
$target = "$env:LOCALAPPDATA\oxdesk\born-on-windows.txt"
"a file born on windows" | Set-Content -Path $target -Encoding utf8
Set-Clipboard -Path $target
