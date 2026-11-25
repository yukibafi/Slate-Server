; Inno Setup script for the Windows installer. Built by .github/workflows/build.yml (or run: iscc installer\create-panel.iss
; after `cargo build --release` in both the root and gui/). Installs for the current user only, no admin prompt.
#define AppName "Slate Server"
#ifndef AppVersion
  #define AppVersion "0.2.13"
#endif
#ifndef BinDir
  #define BinDir "..\target\release"
#endif
#ifndef GuiBinDir
  #define GuiBinDir "..\gui\target\release"
#endif

[Setup]
AppId={{6F1D2C4B-5A7E-4C1B-9E3A-C2EA7E0A9D11}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Slate Server
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=Slate-Server-Setup
SetupIconFile=..\gui\assets\icon.ico
UninstallDisplayIcon={app}\create-panel-app.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Put a shortcut on the desktop"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#BinDir}\create-panel.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#GuiBinDir}\create-panel-app.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\gui\assets\icon.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\create-panel-app.exe"; IconFilename: "{app}\icon.ico"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\create-panel-app.exe"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\create-panel-app.exe"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM create-panel-app.exe"; Flags: runhidden; RunOnceId: "KillApp"
