#!/bin/bash
# Update Slate Server from a new source zip: unpack, build, replace the app, reopen it.
# Usage: double-click, or: ./update.command [path/to/new-zip] [--release]
# Default = fast test build. Add --release for the final optimised build.
# Add --dmg to also make a Slate-Server.dmg (drag-to-Applications disk image) on your Desktop.
# The source and build cache live in ~/.create-panel-build, so only what changed is recompiled
# (the first run is slow, later ones take a short while). It does not matter where this script is.
main() {
  BASE="$HOME/.create-panel-build"; SRC="$BASE/src"; export CARGO_TARGET_DIR="$BASE/target"
  MODE=debug; FLAG=""; ZIP=""; DMG=0
  for x in "$@"; do if [ "$x" = "--release" ]; then MODE=release; FLAG="--release"; elif [ "$x" = "--dmg" ]; then DMG=1; else ZIP="$x"; fi; done
  [ -z "$ZIP" ] && ZIP="$(ls -t "$HOME"/Downloads/create-panel-source*.zip 2>/dev/null | head -1)"
  if [ -z "$ZIP" ] || { [ ! -f "$ZIP" ] && [ ! -d "$ZIP" ]; }; then echo "I can't find a new zip. Put it in Downloads, or run: ./update.command /path/to/zip"; read -r -p "Press Enter to close" _; return 1; fi
  command -v cargo >/dev/null || { [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"; }
  command -v cargo >/dev/null || { echo "Rust (cargo) isn't installed."; read -r -p "Press Enter to close" _; return 1; }
  echo "Using: $ZIP"
  echo "This closes the Slate Server app and its engine while it updates. Stop your Minecraft servers first."
  read -r -p "Continue? (y/N) " a; [ "$a" = "y" ] || [ "$a" = "Y" ] || return 0
  mkdir -p "$SRC"
  if [ -d "$ZIP" ]; then rsync -a --exclude target --exclude .git "$ZIP"/ "$SRC"/ || { echo "Couldn't copy the folder."; read -r -p "Press Enter to close" _; return 1; }
  else unzip -q -o "$ZIP" -d "$SRC" || { echo "Couldn't unzip it."; read -r -p "Press Enter to close" _; return 1; }; fi
  # a zip made by "Compress" may wrap everything in one folder
  [ -f "$SRC/Cargo.toml" ] || { D="$(dirname "$(ls "$SRC"/*/Cargo.toml 2>/dev/null | head -1)")"; [ -n "$D" ] && [ -f "$D/Cargo.toml" ] && SRC="$D"; }
  [ -f "$SRC/Cargo.toml" ] || { echo "That zip doesn't look right."; read -r -p "Press Enter to close" _; return 1; }
  echo "Building ($MODE)..."
  (cd "$SRC" && cargo build $FLAG && cd gui && cargo build $FLAG) || { echo "The build failed. Copy the error above and send it to Claude. Your installed app is unchanged."; read -r -p "Press Enter to close" _; return 1; }
  pkill -f "Slate Server.app/Contents/MacOS" 2>/dev/null; pkill -x create-panel 2>/dev/null; sleep 1
  APP="$HOME/Applications/Slate Server.app"
  mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
  cp "$CARGO_TARGET_DIR/$MODE/create-panel" "$APP/Contents/MacOS/create-panel"
  cp "$CARGO_TARGET_DIR/$MODE/create-panel-app" "$APP/Contents/MacOS/Slate Server"
  cp "$SRC/gui/assets/icon.icns" "$APP/Contents/Resources/icon.icns"
  if [ ! -f "$APP/Contents/Info.plist" ]; then
    cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Slate Server</string>
<key>CFBundleExecutable</key><string>Slate Server</string>
<key>CFBundleIdentifier</key><string>com.createpanel.app</string>
<key>CFBundleIconFile</key><string>icon</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
  fi
  chmod +x "$APP/Contents/MacOS/"*
  codesign --force --deep -s - "$APP" >/dev/null 2>&1
  if [ "$DMG" = "1" ]; then
    STAGE="$BASE/dmg"; rm -rf "$STAGE"; mkdir -p "$STAGE"; cp -R "$APP" "$STAGE/"; ln -s /Applications "$STAGE/Applications"
    rm -f "$HOME/Desktop/Slate-Server.dmg"
    hdiutil create -quiet -volname "Slate Server" -srcfolder "$STAGE" -ov -format UDZO "$HOME/Desktop/Slate-Server.dmg" && echo "Made Slate-Server.dmg on your Desktop."
  fi
  echo "Updated. Opening it..."
  open "$APP"
  return 0
}
main "$@"
exit $?
