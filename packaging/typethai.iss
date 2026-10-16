; Windows installer for typeThai (Inno Setup 6). Per-user, no admin prompt.
; iscc /DAppVersion=0.2.2 /DTag=v0.2.2 /DSrc=<folder with the exe, model and docs> /O<out dir> packaging\typethai.iss
#ifndef AppVersion
  #error Pass /DAppVersion=<x.y.z>
#endif
#ifndef Tag
  #define Tag "v" + AppVersion
#endif
#ifndef Src
  #error Pass /DSrc=<folder with typethai-desktop.exe and typethai.ttm>
#endif

[Setup]
AppId={{1AAF9E1F-896C-4F8C-AA29-BEC465BD1026}
AppName=typeThai
AppVersion={#AppVersion}
AppVerName=typeThai {#AppVersion}
AppPublisher=phasu21
AppPublisherURL=https://github.com/phasu21/typethai
AppSupportURL=https://github.com/phasu21/typethai/issues
DefaultDirName={autopf}\typeThai
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=typeThai-{#Tag}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\typethai-desktop.exe
; typeThai has no window to close, so stop it outright when upgrading.
CloseApplications=force

[Tasks]
Name: autostart; Description: "Start typeThai when I sign in"

[Files]
Source: "{#Src}\typethai-desktop.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Src}\typethai.ttm"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Src}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Src}\DATA_LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Src}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\typeThai"; Filename: "{app}\typethai-desktop.exe"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "typeThai"; \
  ValueData: """{app}\typethai-desktop.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\typethai-desktop.exe"; Description: "Launch typeThai"; Flags: nowait postinstall skipifsilent
; an in-app update runs this installer with /VERYSILENT after typeThai quits: start the new version
Filename: "{app}\typethai-desktop.exe"; Flags: nowait; Check: WizardSilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im typethai-desktop.exe"; Flags: runhidden; RunOnceId: "StopTypeThai"
