#!/bin/bash
# START HERE (Mac): double-click to build and install Slate Server from this folder.
# Also makes Slate-Server.dmg on your Desktop (a drag-to-Applications installer you can keep).
cd "$(dirname "$0")" || exit 1
chmod +x ./update.command 2>/dev/null
exec ./update.command "$(pwd)" --release --dmg
