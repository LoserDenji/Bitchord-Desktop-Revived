; Inno Setup script for BitChord.
; Build: cargo build --release, then
;   iscc installer\bitchord.iss
; ISCC produces the inner payload. Package it with launcher.rs before distributing:
;   $env:BITCHORD_INNO_SETUP = (Resolve-Path installer\Output\bitchord-payload-<version>.exe).Path
;   rustc --edition 2024 -O installer\launcher.rs -o installer\Output\bitchord-setup-<version>.exe
; The version comes from Cargo.toml by default; the release workflow stamps its own:
;   iscc /DAppVersion="2.0.3" installer\bitchord.iss
; ARM64: cargo build --release --target aarch64-pc-windows-msvc, then
;   iscc /DArm64 installer\bitchord.iss
; Add -arm64 to both filenames and --target aarch64-pc-windows-msvc to rustc.

#define AppName "BitChord"
#ifndef AppVersion
#define AppVersion "2.0.2"
#endif
#define AppExe "bitchord.exe"
#ifdef Arm64
  #define Arch "arm64"
  #define Suffix "-arm64"
  #define BinDir "..\target\aarch64-pc-windows-msvc\release"
#else
  #define Arch "x64compatible"
  #define Suffix ""
  #define BinDir "..\target\release"
#endif

[Setup]
; BitChord's own ID, new in 2.0.3. Before that, every install rode on ytm-rs's GUID,
; so Windows treated the two apps as one program: installing either "upgraded" the
; other and they shared a single uninstall entry. An old install under the shared ID
; is uninstalled by [Code] below — its leftover files going with it — so an update
; lands as one program, not two. The data folder is the listener's, and only goes
; when the "cleandata" task below is picked.
AppId={{44C00AFB-5305-4CCA-80C5-5F0D03AC0D29}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Loserdenji
AppPublisherURL=https://github.com/LoserDenji/Bitchord-Desktop-Revived
AppSupportURL=https://github.com/LoserDenji/Bitchord-Desktop-Revived/issues
; A pre-2.0.3 install under the shared AppId may live somewhere custom: keep it.
DefaultDirName={code:DefaultDir}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Per-user install by default (no UAC prompt); the user can pick all-users instead.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed={#Arch}
ArchitecturesInstallIn64BitMode={#Arch}
MinVersion=10.0
OutputBaseFilename=bitchord-payload-{#AppVersion}{#Suffix}
Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\assets\icon\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
CloseApplications=yes
; Restart Manager must not launch the app before its profile has been reset.
RestartApplications=no
SetupLogging=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
; The escape hatch for a broken update: an old settings file (app.ron and friends)
; can leave the app misbehaving after an update, and only wiping the data folder
; fixes it. Checked here, everything under %AppData%\bitchord — settings, caches,
; sign-ins, history, downloads — goes before the new files land, and the app starts
; as a fresh install. Unchecked (the default, and what the in-app updater's silent
; install picks), an update lands on the listener's own data and sign-ins: nobody
; signs in again because the app updated. A pre-2.0.3 install kept its data under
; %AppData%\ytm-rs; that folder is ytm-rs's and is never touched either way.
Name: "cleandata"; Description: "Remove the existing BitChord data before installing (%AppData%\bitchord). Use this if an update left the app misbehaving: it deletes the old settings and caches, so BitChord starts as a fresh install."; GroupDescription: "Fix an update:"; Flags: unchecked

[Files]
; The exe statically links the WebView2 loader, so there's no DLL to ship.
Source: "{#BinDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Dirs]
; Inno processes [Dirs] after closing applications and before replacing files.
Name: "{app}"; BeforeInstall: WipePreviousInstall

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; bitchord:// links (jam and party invites). HKA is HKCU for per-user installs, HKLM
; for all-users. The installed app also re-registers itself under HKCU on launch.
; Until 2.0.3 this also registered ytm-rs's scheme, so BitChord stole ytm-rs's jam
; invites; the stale registration is cleaned up in [Code].
Root: HKA; Subkey: "Software\Classes\bitchord"; ValueType: string; ValueName: ""; ValueData: "URL:bitchord Protocol"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\bitchord"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\bitchord\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"",0"
Root: HKA; Subkey: "Software\Classes\bitchord\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""

[UninstallDelete]
; Installers downloaded by the in-app updater.
Type: filesandordirs; Name: "{userappdata}\bitchord\data\updates"
; The app's own Start menu shortcut on an all-users install, for a custom app icon.
Type: files; Name: "{userprograms}\{#AppName}\{#AppName}.lnk"
Type: dirifempty; Name: "{userprograms}\{#AppName}"

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent; Check: CanLaunchApp

[Code]
const
  // BitChord's own AppId, matching [Setup] above.
  OurAppId = '{44C00AFB-5305-4CCA-80C5-5F0D03AC0D29}';
  // ytm-rs's AppId, which every BitChord install before 2.0.3 also used: Windows
  // treated the two apps as one program. Ours is a new one; an old install under
  // the shared ID is uninstalled here so it doesn't linger beside the new entry.
  LegacyAppId = '{6E0B7C4A-2D1F-4C8E-9B3A-7F5D1E2A9C40}';

var
  // Where a previous BitChord install lives, from its uninstall key's
  // InstallLocation ('' when there is none): the folder whose leftover files the
  // update wipes before the new ones land. A previous uninstaller only removes
  // what its own installer laid down — files renamed or added since stay — so an
  // update that just runs it would mix old files into the new install.
  PrevDir: String;
  ProfileResetSucceeded: Boolean;

// Keep the registry root and view with the first matching value so related
// values can be read from that same registration.
function FindUninstallValue(id, name: String; var root: Integer; var path, value: String): Boolean;
begin
  path := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' + id + '_is1';
  root := HKEY_CURRENT_USER;
  Result := RegQueryStringValue(root, path, name, value);
  if Result then exit;
  root := HKEY_LOCAL_MACHINE;
  Result := RegQueryStringValue(root, path, name, value);
  if Result or not IsWin64 then exit;
  root := HKEY_LOCAL_MACHINE_32;
  Result := RegQueryStringValue(root, path, name, value);
end;

function FindLegacyRegistration(var root: Integer; var path: String): Boolean;
var
  name: String;
begin
  Result := False;
  if not FindUninstallValue(LegacyAppId, 'DisplayName', root, path, name) then exit;
  name := Lowercase(Trim(name));
  // Inno's default display name is AppName or "AppName version AppVersion".
  Result := (name = 'bitchord') or (Pos('bitchord version ', name) = 1);
  if not Result then
    Log('The shared AppId has a different display name; leaving it alone.');
end;

// Accept absolute drive paths and ordinary UNC paths only. Work with the
// expanded path so dot segments and extra separators cannot hide a root.
function NormalizeInstallDir(var dir: String): Boolean;
var
  i: Integer;
  drive, tail: String;
begin
  Result := False;
  dir := Trim(dir);
  StringChangeEx(dir, '/', '\', True);
  drive := Uppercase(Copy(dir, 1, 1));
  if Copy(dir, 1, 2) = '\\' then begin
    if (Copy(dir, 3, 1) = '\') or (Copy(dir, 3, 2) = '.\') then exit;
    tail := Copy(dir, 3, Length(dir));
  end else begin
    if (Length(dir) < 3) or (drive < 'A') or (drive > 'Z') or
       (Copy(dir, 2, 2) <> ':\') then exit;
    tail := Copy(dir, 4, Length(dir));
  end;
  for i := 1 to Length(tail) do
    if (Ord(tail[i]) < 32) or (Pos(tail[i], ':*?"<>|') > 0) then exit;
  dir := RemoveBackslashUnlessRoot(ExpandFileName(dir));
  drive := ExtractFileDrive(dir);
  if (drive = '') or (CompareText(AddBackslash(dir), AddBackslash(drive)) = 0) then exit;

  // Win32 can treat trailing dots and spaces as aliases for another directory.
  tail := AddBackslash(Copy(dir, Length(drive) + 1, Length(dir)));
  if (Pos('.\', tail) > 0) or (Pos(' \', tail) > 0) then exit;
  Result := True;
end;

// Keep a valid custom directory only when it belongs to legacy BitChord.
function DefaultDir(Param: String): String;
var
  root: Integer;
  path, location: String;
begin
  Result := ExpandConstant('{autopf}\{#AppName}');
  if not FindLegacyRegistration(root, path) then exit;
  if RegQueryStringValue(root, path, 'InstallLocation', location) and
     NormalizeInstallDir(location) then
    Result := location;
end;

// HKEY_AUTO uses the selected install mode's root and default registry view,
// just like the uninstall registration Setup writes.
function InitializeSetup(): Boolean;
begin
  PrevDir := '';
  // Nothing to reset unless the "cleandata" task asks for it: the launch after
  // install is allowed, and ResetInstallProfile takes that away only on failure.
  ProfileResetSucceeded := True;
  RegQueryStringValue(HKEY_AUTO,
    'Software\Microsoft\Windows\CurrentVersion\Uninstall\' + OurAppId + '_is1',
    'InstallLocation', PrevDir);
  Result := True;
end;

// Split "…unins000.exe" /flags into the exe and its flags, then run it with ours.
procedure RunUninstaller(unins: String);
var
  p: Integer;
  exe, params: String;
  code: Integer;
  i: Integer;
begin
  unins := Trim(unins);
  if Pos('"', unins) = 1 then begin
    p := Pos('" ', unins);
    if p = 0 then begin
      // Just "…unins000.exe", no flags after it.
      exe := Copy(unins, 2, Length(unins) - 2);
      params := '';
    end else begin
      exe := Copy(unins, 2, p - 2);
      params := Copy(unins, p + 2, Length(unins) - p - 1);
    end;
  end else begin
    p := Pos(' ', unins);
    if p = 0 then begin
      exe := unins;
      params := '';
    end else begin
      exe := Copy(unins, 1, p - 1);
      params := Copy(unins, p + 1, Length(unins) - p);
    end;
  end;
  Log('Running the old uninstaller: ' + exe + ' ' + params);
  Exec(exe, params + ' /VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_SHOW, ewWaitUntilTerminated, code);
  // The uninstaller runs from a temp copy of itself and returns before that copy
  // is done, so wait for the old files to be gone before the new ones land.
  for i := 0 to 59 do begin
    if not FileExists(exe) then exit;
    Sleep(250);
  end;
  Log('The old uninstaller is taking its time; carrying on.');
end;

function GetFileAttributes(name: String): LongWord;
  external 'GetFileAttributesW@kernel32.dll stdcall';

procedure WipeLeftovers(dir: String);
var
  parent, next, root: String;
  attributes: LongWord;
begin
  if dir = '' then exit;
  if not NormalizeInstallDir(dir) then begin
    Log('Skipping leftover cleanup: unsupported install path or root.');
    exit;
  end;
  if not DirExists(dir) then exit;
  // The wildcard passed to DelTree would follow a junction in the base path.
  parent := dir;
  root := AddBackslash(ExtractFileDrive(dir));
  while CompareText(AddBackslash(parent), root) <> 0 do begin
    attributes := GetFileAttributes(parent);
    if (attributes = $FFFFFFFF) or ((attributes and $400) <> 0) then begin
      Log('Skipping leftover cleanup: cannot safely traverse ' + parent);
      exit;
    end;
    next := ExtractFileDir(parent);
    if (next = '') or (next = parent) then exit;
    parent := next;
  end;
  if DelTree(AddBackslash(dir) + '*', False, True, True) then
    Log('Removed an install''s leftover files at ' + dir)
  else
    Log('Could not remove all leftover files at ' + dir);
end;

// The uninstaller can erase its registration, so capture its directory first.
procedure RemoveLegacyInstall;
var
  root: Integer;
  path, unins, location: String;
begin
  if not FindLegacyRegistration(root, path) then exit;
  if not RegQueryStringValue(root, path, 'QuietUninstallString', unins) or (unins = '') then
    if not RegQueryStringValue(root, path, 'UninstallString', unins) or (unins = '') then begin
      Log('No uninstaller for the old install under the shared AppId; skipping.');
      exit;
    end;
  if not RegQueryStringValue(root, path, 'InstallLocation', location) then
    location := '';
  RunUninstaller(unins);
  WipeLeftovers(location);
end;

procedure RegisterExtraCloseApplicationsResources;
var
  dir: String;
begin
  // The [Files] entry covers the new destination, which can differ from PrevDir.
  dir := PrevDir;
  if NormalizeInstallDir(dir) then
    RegisterExtraCloseApplicationsResource(Is64BitInstallMode, AddBackslash(dir) + '{#AppExe}');
end;

procedure WipePreviousInstall;
begin
  WipeLeftovers(PrevDir);
end;

// The ytm-rs:// class registration a pre-2.0.3 install wrote: give the scheme back
// to ytm-rs, but only where it opens a BitChord exe. A registration pointing at
// ytm-rs's own exe is ytm-rs's, and stays. HKLM needs the elevated half of an
// all-users install; where it isn't writable, the app's own launch does the HKCU
// half (see deeplink.rs).
procedure RemoveStaleSchemeAt(root: Integer);
var
  cmd: String;
begin
  if RegQueryStringValue(root, 'Software\Classes\ytm-rs\shell\open\command', '', cmd) then
    if Pos('bitchord.exe', Lowercase(cmd)) > 0 then begin
      RegDeleteKeyIncludingSubkeys(root, 'Software\Classes\ytm-rs');
      Log('Removed this app''s ytm-rs:// registration.');
    end;
end;

procedure RemoveStaleScheme;
begin
  RemoveStaleSchemeAt(HKEY_CURRENT_USER);
  RemoveStaleSchemeAt(HKEY_LOCAL_MACHINE);
end;

function CanLaunchApp(): Boolean;
begin
  Result := ProfileResetSucceeded;
end;

function GetCustomSetupExitCode(): Integer;
begin
  Result := 0;
  if not ProfileResetSucceeded then
    Result := 10;
end;

procedure ResetInstallProfile;
var
  code: Integer;
  failure: String;
begin
  // Only the "Remove the existing BitChord data" task wipes the profile and the
  // saved sign-ins: an update (and the updater's silent install) keeps them, so a
  // listener doesn't land signed out of everywhere because the app updated.
  if not IsTaskSelected('cleandata') then begin
    Log('Keeping the existing data and sign-ins.');
    exit;
  end;
  if not ExecAsOriginalUser(ExpandConstant('{app}\{#AppExe}'), '--reset-install-profile',
      '', SW_HIDE, ewWaitUntilTerminated, code) then
    failure := 'Could not start the profile reset: ' + SysErrorMessage(code)
  else if code <> 0 then
    failure := 'Profile reset exited with code ' + IntToStr(code) + '.'
  else begin
    ProfileResetSucceeded := True;
    exit;
  end;
  ProfileResetSucceeded := False;
  Log(failure);
  SuppressibleMsgBox(failure + #13#10 +
    'BitChord was installed but will not be launched. Close BitChord and rerun Setup without starting it as administrator.',
    mbError, MB_OK, IDOK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then begin
    RemoveLegacyInstall();
    RemoveStaleScheme();
  end else if CurStep = ssPostInstall then
    ResetInstallProfile();
end;

// The app's close button hides it to the tray, so while it runs its exe is locked
// and the uninstaller would leave it — and the folder holding it — behind. End it
// before any file goes; its writes are tmp-and-rename, so a kill mid-write is safe.
procedure CloseRunningApp;
var
  code: Integer;
begin
  Exec(ExpandConstant('{cmd}'), '/D /C taskkill /IM "{#AppExe}" /T /F >NUL 2>&1', '',
    SW_HIDE, ewWaitUntilTerminated, code);
end;

// Uninstall asks before the data goes: settings, sign-ins, listening history and
// downloads are the user's, and a later install can still use them. Answered yes,
// the same reset the "cleandata" task runs takes them, while the exe is still
// there to run it. A silent uninstall keeps them (no dialog to ask with).
procedure RemoveDataIfAsked;
var
  code: Integer;
begin
  if SuppressibleMsgBox('Remove BitChord''s data as well? Settings, sign-ins, ' +
    'listening history and downloads under %AppData%\bitchord are removed; they ' +
    'otherwise stay, and a later install picks them up.',
    mbConfirmation, MB_YESNO, IDNO) <> IDYES then
    exit;
  if (not ExecAsOriginalUser(ExpandConstant('{app}\{#AppExe}'), '--reset-install-profile',
      '', SW_HIDE, ewWaitUntilTerminated, code)) or (code <> 0) then
    Log('The data reset failed; leaving the data in place.');
end;

procedure CurUninstallStepChanged(CurStep: TUninstallStep);
begin
  // Before the files go: the app has to be gone for its exe to be deletable, and
  // the reset has to run while the exe still exists.
  if CurStep = usUninstall then begin
    CloseRunningApp();
    RemoveDataIfAsked();
  end;
end;
