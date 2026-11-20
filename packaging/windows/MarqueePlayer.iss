; Marquee Player installer (Inno Setup 6.7). Built by `cargo xtask release` (crates/xtask):
;   ISCC.exe /DAppVersion=<version> /DSourceDir=<staged files> /DIconFile=<.ico> /O<dist> MarqueePlayer.iss
; SourceDir holds MarqueePlayer.exe, the Visual C++ runtime DLLs beside it, README.md,
; LICENSE.txt and THIRD-PARTY-NOTICES.txt (the portable folder without portable.txt).
;
; What it does (docs/releases.md, "Editions"):
; - installs for the user (no administrator rights, %LOCALAPPDATA%\Programs) or for everyone
;   (C:\Program Files), chosen in the first dialog, or with /CURRENTUSER or /ALLUSERS; an
;   update keeps the scope it was installed with;
; - registers the app the documented way for its scope (HKCU or HKLM): a ProgID per format,
;   Applications\MarqueePlayer.exe with SupportedTypes, OpenWithProgids, and Capabilities
;   under RegisteredApplications, so it is in "Open with" and in Default apps, taking no
;   association; App Paths, so Win+R "MarqueePlayer" starts it;
; - offers, unchecked, to open Windows' Default apps page for it at the end (Windows does not
;   let an installer take the defaults itself), and checked, to start it;
; - closes a running installed copy cleanly before replacing it (the app's quit event), and
;   the uninstaller does the same; the uninstaller removes everything registered here and the
;   app's "Start with Windows" entry, and asks (default No) whether to delete the library
;   data too. Music folders are never touched.
; The app looks for these names (platform/windows/integration.rs; a unit test reads this file).

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=YYYY.M.D.N
#endif
#ifndef SourceDir
  #error Pass the staged files: ISCC /DSourceDir=<folder>
#endif
#ifndef IconFile
  #error Pass the setup icon: ISCC /DIconFile=<file.ico>
#endif

#define AppName "Marquee Player"
#define AppExe "MarqueePlayer.exe"
#define AppId "{D26BA63F-38FE-47B8-8D56-1BB847544A6E}"
#define Publisher "Offbeat"
#define Copyright "Copyright © 2026 Offbeat"
; Every format the scanner reads (library::scan::AUDIO_EXTS).
#define Extensions "opus;ogg;oga;flac;mp3;mp2;wav;aif;aiff;aifc;aac;m4a;m4b;mp4"
#define Capabilities "Software\Offbeat\Marquee Player\Capabilities"

[Setup]
AppId={{#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#Publisher}
AppCopyright={#Copyright}
VersionInfoVersion={#AppVersion}
VersionInfoProductVersion={#AppVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoTextVersion={#AppVersion}
VersionInfoCompany={#Publisher}
VersionInfoCopyright={#Copyright}
VersionInfoDescription={#AppName} Setup
VersionInfoProductName={#AppName}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
; For the user by default, without administrator rights; "for all users" is offered in the
; first dialog (and /ALLUSERS), and asks for elevation only then.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
UsePreviousPrivileges=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1809 or later (docs/releases.md, "Windows 10").
MinVersion=10.0.17763
LicenseFile={#SourceDir}\LICENSE.txt
SetupIconFile={#IconFile}
WizardStyle=modern
ShowLanguageDialog=no
OutputBaseFilename=MarqueePlayer-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
ChangesAssociations=yes
SetupMutex=MarqueePlayerSetup,Global\MarqueePlayerSetup
; A running copy is asked to quit first ([Code]); Restart Manager is the fallback for anything
; else holding the files, and the app is not restarted behind the user's back.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
; The Visual C++ runtime, app-local (beside the exe), so no separate redistributable is needed.
Source: "{#SourceDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Play the music on your drives"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The application: "Open with", and how the app knows it is installed.
Root: HKA; Subkey: "Software\Classes\Applications\MarqueePlayer.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\MarqueePlayer.exe\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"",0"
Root: HKA; Subkey: "Software\Classes\Applications\MarqueePlayer.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
; Default apps: the capabilities, registered under the name the app opens the page with.
Root: HKA; Subkey: "Software\Offbeat\Marquee Player"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Offbeat"; Flags: uninsdeletekeyifempty
Root: HKA; Subkey: "{#Capabilities}"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"
Root: HKA; Subkey: "{#Capabilities}"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "A music player for the music on your own drives, album art first."
Root: HKA; Subkey: "{#Capabilities}"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: """{app}\{#AppExe}"",0"
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#AppName}"; ValueData: "{#Capabilities}"; Flags: uninsdeletevalue
; Win+R "MarqueePlayer".
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExe}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe}"; Flags: uninsdeletekey
; Each format: its own ProgID (so Explorer still names the type when the app is the default),
; listed in the extension's OpenWithProgids, the application's SupportedTypes and the
; capabilities' FileAssociations. No extension's default is changed.
#define Rest
#define Ext
#define Desc
#sub EmitFormat
  #define Desc Ext == "opus" ? "Opus audio" : Ext == "ogg" ? "Ogg Vorbis audio" : Ext == "oga" ? "Ogg audio" : \
    Ext == "flac" ? "FLAC audio" : Ext == "mp3" ? "MP3 audio" : Ext == "mp2" ? "MP2 audio" : Ext == "wav" ? "WAV audio" : \
    Ext == "aif" || Ext == "aiff" || Ext == "aifc" ? "AIFF audio" : Ext == "aac" ? "AAC audio" : \
    Ext == "m4b" ? "MPEG-4 audiobook" : "MPEG-4 audio"
Root: HKA; Subkey: "Software\Classes\MarqueePlayer.{#Ext}"; ValueType: string; ValueName: ""; ValueData: "{#Desc}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\MarqueePlayer.{#Ext}"; ValueType: string; ValueName: "FriendlyTypeName"; ValueData: "{#Desc}"
Root: HKA; Subkey: "Software\Classes\MarqueePlayer.{#Ext}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"",0"
Root: HKA; Subkey: "Software\Classes\MarqueePlayer.{#Ext}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.{#Ext}\OpenWithProgids"; ValueType: string; ValueName: "MarqueePlayer.{#Ext}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\MarqueePlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".{#Ext}"; ValueData: ""
Root: HKA; Subkey: "{#Capabilities}\FileAssociations"; ValueType: string; ValueName: ".{#Ext}"; ValueData: "MarqueePlayer.{#Ext}"
#endsub
#for {Rest = Extensions + ";"; Len(Rest) > 0; Rest = Copy(Rest, Pos(";", Rest) + 1)} Ext = Copy(Rest, 1, Pos(";", Rest) - 1), EmitFormat

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start Marquee Player"; Flags: postinstall nowait skipifsilent
; Windows lets only the user choose default apps: this opens the page where they do. The
; address names the registration for the scope installed (Windows 10 opens the Default apps
; page itself).
Filename: "ms-settings:defaultapps?registeredAppUser=Marquee%20Player"; Description: "Make Marquee Player the default for music files (opens Windows' Default apps settings)"; Flags: postinstall shellexec nowait skipifsilent unchecked; Check: not IsAdminInstallMode
Filename: "ms-settings:defaultapps?registeredAppMachine=Marquee%20Player"; Description: "Make Marquee Player the default for music files (opens Windows' Default apps settings)"; Flags: postinstall shellexec nowait skipifsilent unchecked; Check: IsAdminInstallMode

[Code]
const
  // The app's names (platform/windows/integration.rs).
  InstalledMutex = 'MarqueePlayer.Installed';
  QuitEventName = 'Local\MarqueePlayer.Installed.Quit';
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  ApprovedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run';
  RunValue = 'Marquee Player';
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1';
  EVENT_MODIFY_STATE = $0002;

function OpenEvent(DesiredAccess: DWORD; InheritHandle: BOOL; Name: String): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function SetEvent(Event: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

// The installed app runs in this session (it holds the session's mutex).
function RunningHere(): Boolean;
begin
  Result := CheckForMutexes(InstalledMutex);
end;

// Ask every installed copy in this session to quit (it saves its queue and settings first),
// and wait up to 10 seconds for them to go.
function AskToQuit(): Boolean;
var
  Event: THandle;
  I: Integer;
begin
  Event := OpenEvent(EVENT_MODIFY_STATE, False, QuitEventName);
  if Event <> 0 then
  begin
    SetEvent(Event);
    CloseHandle(Event);
  end;
  for I := 1 to 100 do
  begin
    if not RunningHere() then
    begin
      Result := True;
      Exit;
    end;
    Sleep(100);
  end;
  Result := not RunningHere();
end;

// Close a running copy before files are replaced or removed. False: the user cancelled.
function CloseRunningApp(Silent: Boolean; What: String): Boolean;
begin
  Result := True;
  while RunningHere() do
  begin
    if not Silent then
      if SuppressibleMsgBox('Marquee Player is running. ' + What + ' will close it now; it saves your queue and where you were first.',
           mbConfirmation, MB_OKCANCEL, IDOK) = IDCANCEL then
      begin
        Result := False;
        Exit;
      end;
    if not AskToQuit() then
      if SuppressibleMsgBox('Marquee Player did not close. Quit it from its tray icon (right-click it, then Quit), then choose Retry.',
           mbError, MB_RETRYCANCEL, IDCANCEL) = IDCANCEL then
      begin
        Result := False;
        Exit;
      end;
  end;
  // Installed for everyone: another user signed in on this computer may be playing.
  while IsAdminInstallMode and CheckForMutexes('Global\' + InstalledMutex) do
    case SuppressibleMsgBox('Marquee Player is running for another user signed in on this computer. Ask them to quit it, then choose Retry.' + #13#10#13#10 +
           'Ignore goes on anyway: files that are in use are replaced when the computer restarts.', mbError, MB_ABORTRETRYIGNORE, IDIGNORE) of
      IDABORT: begin Result := False; Exit; end;
      IDIGNORE: Exit;
    end;
end;

// One installation per user: for-me-only and for-everyone of the same app would both
// register it, and the user's would hide the other.
function InitializeSetup(): Boolean;
begin
  Result := True;
  if IsAdminInstallMode and RegKeyExists(HKCU, UninstallKey) then
  begin
    SuppressibleMsgBox('Marquee Player is already installed for you only. To update it, run Setup again and choose "Install for me only".' + #13#10#13#10 +
      'To install it for all users instead, first uninstall it in Settings > Apps > Installed apps (your library is kept).', mbError, MB_OK, IDOK);
    Result := False;
  end
  else if not IsAdminInstallMode and RegKeyExists(HKLM64, UninstallKey) then
  begin
    SuppressibleMsgBox('Marquee Player is already installed for all users on this computer. To update it, run Setup again and choose "Install for all users".' + #13#10#13#10 +
      'To install it for you only instead, first uninstall it in Settings > Apps > Installed apps (your library is kept).', mbError, MB_OK, IDOK);
    Result := False;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not CloseRunningApp(WizardSilent, 'Setup') then
    Result := 'Marquee Player is still running. Quit it from its tray icon, then run Setup again.';
end;

function InitializeUninstall(): Boolean;
begin
  Result := CloseRunningApp(UninstallSilent, 'Uninstall');
end;

// The app's "Start with Windows" entry, when it starts this installation's exe.
procedure RemoveStartupEntry();
var
  Command: String;
begin
  if RegQueryStringValue(HKCU, RunKey, RunValue, Command) and
     (Pos(Lowercase(ExpandConstant('{app}\{#AppExe}')), Lowercase(Command)) > 0) then
  begin
    RegDeleteValue(HKCU, RunKey, RunValue);
    RegDeleteValue(HKCU, ApprovedKey, RunValue);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Roaming, Local: String;
begin
  if CurUninstallStep = usUninstall then
    RemoveStartupEntry();
  // The library is the user's: kept unless they say otherwise, never in a silent uninstall.
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
  begin
    Roaming := ExpandConstant('{userappdata}\MarqueePlayer');
    Local := ExpandConstant('{localappdata}\MarqueePlayer');
    if DirExists(Roaming) or DirExists(Local) then
      if MsgBox('Also delete your Marquee Player library and settings?' + #13#10#13#10 +
           'Your play history, playlists, favourites and settings would be deleted for good (' + Roaming + ' and ' + Local + '). ' +
           'Your music files are not touched either way.' + #13#10#13#10 +
           'Choose No to keep them for a later installation.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      begin
        DelTree(Roaming, True, True, True);
        DelTree(Local, True, True, True);
      end;
  end;
end;
