# Slate Server (Rust)

# Start here (install in one click)
- **Mac:** unzip, double-click `Install.command` (right-click > Open the first time). It builds the app, puts it in Applications, and can make a `.dmg`.
- **Windows:** unzip, double-click `Install.bat`. It builds the app and runs the `Slate-Server-Setup.exe` installer.
- Both need internet the first time (Rust toolchain; the scripts fetch it).

## Prebuilt .exe / .dmg via GitHub (no local build)
1. Create a new GitHub repo and push the contents of this zip to it.
2. Open the repo's **Actions** tab, run **Build apps** (or push a tag like `v0.1.0`).
3. Download `Slate-Server-Setup.exe` (Windows) and the `.dmg` (Mac) from the run's Artifacts, or from Releases if you pushed a tag.


One binary. No dependencies (std only). Runs as:
- **Hub** (default): the Server Hub page, supervises one panel process per server.
- **Server panel**: `PANEL_ROLE=server` or `--server` (the hub starts these for you).

There is **no website any more**. The binary is the engine; the desktop app (Slint, in `gui/`) is the only interface. A browser pointed at the engine just shows a one-line notice. The key is printed at start and stored in `~/.create-panel-rs/key`.

## Architecture
- `create_panel` (this crate, std only): engine + JSON API, runs Minecraft servers, upload-update.
- `client.rs`: tiny HTTP client the app uses. The Mac app runs its own engine and talks to it on localhost; the **Windows app connects to the Mac's engine** (address + key) and shows/controls the same servers, so both always agree. Same code path for both, so it can be tested without a screen.
- `gui/` (separate crate, needs the `slint` crate): the native screens (Deepslate theme, hub + server views).
- `legacy-web-ui/`: the old HTML pages, kept only as a reference for what each screen shows. Not built or served.

## Build
    cargo build --release        # on the OS you want the app for
    ./target/release/create-panel
Build once per target (macOS arm64, macOS x86_64, Windows, Linux). Only Linux was built and tested here.

## Theme
One theme (Deepslate, 8 palettes, fonts) shared by the hub and every server panel. It is stored on the server in `~/.create-panel-rs/theme.json`, so changing it anywhere changes it everywhere.

## Update by upload
Build the new version (bump `version` in Cargo.toml), then drop the file on the hub page, or:
    curl -X POST -H "X-Key: KEY" --data-binary @create-panel "http://HOST:8080/api/panel/upgrade"
Add `?check=1` to dry-run, `?force=1` to allow same/older. It checks the file is a real build for this OS/CPU and newer, self-tests it, stops Minecraft cleanly, swaps (old one kept as `create-panel.prev`), restarts, and rolls back if the new one doesn't answer within 25 s. Child panels are restarted onto the new version automatically.

## Ported
Everything the old Python panel did, except the old Discord *bot* (it needed Python; Discord webhook alerts are kept):
server start/stop/restart/kill, console + commands, players/bans/lists, Settings (server.properties, RAM, Java, flags), mods (upload, enable/disable, remove, mod browser for Modrinth + CurseForge with dependencies, update check + safe install, update schedule), modpack export (zip / Modrinth / CurseForge / Prism) and add-a-modpack, compare with launcher, files, backups, history graphs, automation, crash auto-restart, Java download, loader updates (Fabric/Quilt/NeoForge/Forge/Paper), network checks, start-at-login, hub (new server, import, rename, remove, first-run setup), import from the old panel, upload-update.
The hub also forwards `/api/s/<server id>/...` to each server, so the Windows app only needs the Mac's one address.

## Not done / known gaps
- Intentionally not included: keyboard shortcuts footer, web link.
- The Slint UI and Windows scripts/installer have not been compiled or run on real machines yet; the first build may need small fixes.
- Mac-only features (firewall check/fix, start at login, keep awake with lid closed, osascript prompts) can only be confirmed on a real Mac.

## Updating (no reinstall)
Each build run on GitHub also produces an **update file** (`create-panel-update-<platform>`, in the same artifact as the installer). In the app: Settings > **Update from a file...**, pick that file. It updates the engine, and the engine then replaces the app window with the matching version. Close and reopen the app afterwards. Pick the file for your computer (mac-apple-silicon, mac-intel or windows).

## Also in the app
- Tools tab > Server software > **Choose a specific version**: pick the Minecraft version and exact Fabric/Forge build.
- First launch shows a **setup guide** (Java download, Homebrew on Mac, restore a Server Hub Backup). Reopen it from Settings > Setup guide.
- Settings > **Update engine from a file...** installs a newer engine build (checked, self-tested, rolls back if it doesn't start).
- Hub > Discord bot page has **Server alerts (webhook)** for every server.
- **Theme:** Settings has a real colour picker: type any hex code or drag Hue / Intensity / Brightness, pick a surface style (Graphite, Tinted, Black), or use a quick-start swatch. Saved on the engine, so every computer shows it.
- **Console:** click any line to copy it, "Select text" mode to highlight part of it, Copy shown / Save log buttons, and an **AutoModpack fingerprint** card (with a Get fingerprint button) so you can paste it straight into the client.
- **Files:** Jump-to chips (mods, config, logs, crash-reports, world) and a Show filter (Folders, Config, Logs, Mods, Archives, Images).
- **Mods:** Copy mod list, Sync AutoModpack, and Leave alone (selected mods never get update offers; edit the list in Settings > Automation > Mod updates).
- **Players:** Announce box sends a chat message to everyone online. **Hub:** search box appears once you have more than three servers; the address for Windows also shows a name that doesn't change when the IP does.
- **New look (0.2.4):** left sidebar on the hub and inside each server, banner server cards with the join address and a Copy button, status pills, metric tiles on Overview (players, memory, uptime, last backup), and one header with Start / Restart / Stop / Back up now. Pick any colour under Settings and everything follows it.
- **One-click updates (0.2.5):** Settings > Updates > **Check now / Update now**. The app looks at the newest GitHub Release of yukibafi/Slate-Server, downloads the file for your computer and installs it (engine and app on the Mac that hosts, app only on Windows, which replaces itself and reopens). A small banner on the hub also appears when something newer is out. For a private repo, paste a GitHub token in the box that shows up. Builds (installer + update files + a GitHub Release) now run automatically on every push; re-paste `.github/workflows/build.yml` once because the website upload skips hidden folders. Older builds without this still need one manual "Update from a file..." / reinstall first.
- **0.2.13:** New logo (a stack of server slabs, replacing the plain S) and the Inter font is bundled so text looks the same on Mac and Windows. **Updater fixed.** The app now asks the engine itself whether its files are ready (so a Windows app talking to a Mac engine checks the right file), falls back to the releases list when GitHub's "latest" answers 404, and says exactly what is missing. Settings, Updates now shows the app version and an editable **GitHub repository** box (default `yukibafi/Slate-Server`); saving it also tells the engine. Apps built before 0.2.10 still look at the old repository, so update those once by hand (Update from a file or reinstall). **Look** page rebuilt: live mini preview, 20 accent colour cards, three surface preview cards, and a fold-out for hex/sliders. **Backups** page rebuilt with stat tiles and properly sized cards (no more clipped buttons). **Discord bot** page rebuilt as a status card, a 3-step set-up, feature toggles and server alerts. **Servers (hub home)** gets tiles (servers online, players, memory, last backup), a Who's online list and Quick actions that fill the space under your one or two servers.
- **0.2.12:** renamed to **Slate Server** with a new icon (also set as the window icon, so it shows in the Windows taskbar). Overview tiles now match the concept: Players with a graph and "of 20", Memory, **Ticks per second** with a graph (the panel asks the server for its TPS once a minute and hides the reply from the console), and **World size** with free disk. Who's on and Quick actions also sit beside the console on the Console tab when the window is wide. Backups, Datapacks and the add-server / import screens use the new style and sidebar.
- **0.2.11:**
  - Mod browser: live search (results update as you type), bigger cards with a short description, a Details page that shows what the mod does, Uninstall right on any installed mod, tick-boxes for installing or removing several mods at once.
  - Console lines are split into time / level / message columns, with warnings and errors highlighted.
  - Settings is now a category list on the left (server properties, Look, Automation & updates, Manage server) with one "Unsaved changes" bar. The empty Options tab is gone; its rename / crash restart / force stop / remove controls are under Manage server.
  - Players tab has two views (Players / Lists & add) and scrolls properly; Settings and Tools pages scroll as one page.
- **Players tab (0.2.9):** counts for online / whitelisted / operators / banned, a table of everyone who has played (playtime, joins, last seen, Op / Whitelisted / Banned tags) with Op, Whitelist, Kick and Ban buttons on each row, and Remove / Pardon buttons on the whitelist, operator and ban lists. Overview still shows the quick player stats.
- **Menus match the concept (0.2.8):** Overview now has the live console with filter chips and a command box, Who's on with Announce, and Quick actions beside it; Files, Players, Backups, Tools, Settings and Options use the same headers and spacing, and wide toolbars split into rows so nothing is cut off.
- **Stuck panel port fixed (0.2.7):** if another program is using a server panel's internal port, the hub now moves that panel to a free port by itself instead of staying on "connecting"/"couldn't connect".
- **Why a server shows "panel off" (0.2.6):** the hub now says the reason on the server card (for example an older panel still holding the port, or what the panel printed before it closed). Each server panel also writes `~/.create-panel-rs/logs/panel-<id>.log`.
- **Smaller windows (0.2.5):** header, console toolbar, mods toolbar and quick actions wrap instead of being cut off; Backups, Discord bot and Settings share the hub sidebar.
- **Discord /start:** admins of your Discord server type `/start`, pick a server from the list and it turns on (it says when it's online). There is no stop command. The bot gets its own start-only key, so it can never stop, delete or change anything. Switch it off under Hub > Discord bot > Options. After updating, restart the bot once so Discord picks up the new command.
- Destructive buttons (force stop, remove, delete, quit engine) ask for a second click.

## Desktop app (gui/)
Slint app that shows the engine's state. Build both crates:
    cargo build --release                      # engine -> target/release/create-panel
    cd gui && cargo build --release            # app    -> gui/target/release/create-panel-app
Keep `create-panel` next to the app (the Mac .app puts both in Contents/MacOS). Use the GitHub Action in `.github/workflows/build.yml` to get ready-made Mac (Apple Silicon + Intel) and Windows zips.

## Updating
Drop the new `create-panel-source.zip` into Downloads and double-click `update.command` (or run `./update.command /path/to/zip`). It unpacks, builds, replaces `~/Applications/Slate Server.app` and reopens it. On Windows, double-click `update.bat` (it runs `update.ps1`: builds from the newest zip in Downloads, installs to `%LOCALAPPDATA%\Programs\Slate Server`, adds a Start Menu shortcut, opens it; needs Rust + the C++ build tools once). For a normal installer, the GitHub workflow also produces `Slate-Server-Setup.exe` from `installer/create-panel.iss` (Inno Setup, per-user install, no admin prompt).

Installers: `update.ps1 -Installer` (Windows) builds and runs a real `Slate-Server-Setup.exe`; `./update.command --dmg` (Mac) puts a drag-to-Applications `Slate-Server.dmg` on the Desktop.

## Discord bot, player stats, skins
Hub page > **Discord bot**: install (private Python venv), paste the token, copy the invite link, start/stop, toggle /modlist, /fingerprint, /welcome, set allowed user IDs, read the log. Players page shows play time, joins and last seen per player, with head avatars (from skins) in every list.

## App pages
Overview (stats, connect + copy, public IP, quick actions, Update NeoForge, players, activity, history graph), Console (filters, auto-scroll, copy log, quick commands), Mods (installed list with select/bulk/sort, Browse Modrinth/CurseForge, Add mods, modpack export), Datapacks, Files (browse, edit, upload, download, rename, delete), Players, Backups, Settings (server + Java memory, Automation & updates), Options.
