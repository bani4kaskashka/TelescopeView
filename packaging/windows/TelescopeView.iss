; Windows installer (Inno Setup 6). Built by packaging/build-windows.ps1, which passes:
;   /DVersion=0.1.0  /DArch=x64|arm64  /DSource=<folder with TelescopeView.exe and ffmpeg\>
;   /DOutDir=<where the setup .exe goes>
; Installs for the current user (no admin prompt) into %LOCALAPPDATA%\Programs\TelescopeView.

#define Name "TelescopeView"

[Setup]
AppId={{6E1C2B7A-4F0D-4C55-9C1B-7A5D3F2E8B41}
AppName={#Name}
AppVersion={#Version}
AppVerName={#Name} {#Version}
AppPublisher={#Name}
AppPublisherURL=https://github.com/bani4kaskashka/TelescopeView
AppSupportURL=https://github.com/bani4kaskashka/TelescopeView/issues
DefaultDirName={autopf}\{#Name}
DefaultGroupName={#Name}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
OutputDir={#OutDir}
OutputBaseFilename={#Name}-{#Version}-windows-{#Arch}-setup
SetupIconFile=..\..\assets\app.ico
UninstallDisplayIcon={app}\{#Name}.exe
UninstallDisplayName={#Name}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; A running copy is closed (and reopened) when updating.
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\{#Name}.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Source}\ffmpeg\*"; DestDir: "{app}\ffmpeg"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#Name}"; Filename: "{app}\{#Name}.exe"
Name: "{autodesktop}\{#Name}"; Filename: "{app}\{#Name}.exe"; Tasks: desktopicon

[Run]
; "Open with" and Default apps know about it right away, before the first start.
Filename: "{app}\{#Name}.exe"; Parameters: "--register"; Flags: runhidden waituntilterminated
Filename: "{app}\{#Name}.exe"; Description: "{cm:LaunchProgram,{#Name}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Takes its "Open with" / Default apps entries back out of the registry.
Filename: "{app}\{#Name}.exe"; Parameters: "--unregister"; Flags: runhidden waituntilterminated; RunOnceId: "Unregister"
