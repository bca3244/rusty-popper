; Rusty Popper installer (Inno Setup 6.3 or newer).
;
; Built by .github/workflows/installer.yml from a release's
; rusty-popper-vX-windows-x64.zip, so it installs exactly the files the zip
; (and the in-app updater) ships. To build it by hand, unzip a release and run:
;
;   ISCC /DAppVersion=0.2.59 /DSourceDir=C:\path\to\rusty-popper /DOutputDir=C:\out installer\rusty-popper.iss
;
; What it does:
; - finds PinUP Popper the way the app does (the default C:\vPinball\PinUPSystem,
;   then PinUpPlayer's registration, then the usual folders on every drive);
;   the folder can be changed on its own page, or with /POPPER=<dir>
; - installs to a RustyPopper folder beside PinUPSystem (C:\vPinball\RustyPopper)
; - upgrades in place: program files are replaced, rusty-popper.toml is kept
; - closes a running Rusty Popper first
; - optional: desktop shortcut (on), start at boot instead of Popper (off)

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\rusty-popper"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

#define AppName "Rusty Popper"
#define AppExe "rusty-popper.exe"
#define RepoUrl "https://github.com/bca3244/rusty-popper"

[Setup]
AppId={{D9D36780-25DF-497C-B3F9-84DE07204371}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Rusty Popper contributors
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={code:DefaultInstallDir}
UsePreviousAppDir=yes
; Skip the folder page when upgrading an install made by this setup.
DisableDirPage=auto
DisableProgramGroupPage=yes
DefaultGroupName={#AppName}
; The app writes next to its exe (settings, logo cache, updates), so it has
; to live in a folder the user can write to: no admin install.
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=RustyPopper-Setup
SetupIconFile=..\crates\rp-app\assets\rusty-popper.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; Setup closes Rusty Popper itself (PrepareToInstall), not via Restart Manager.
CloseApplications=no

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"
Name: "startup"; Description: "&Start Rusty Popper at boot instead of PinUP Popper (asks for administrator rights)"; Flags: unchecked

[InstallDelete]
; A previous in-app update's leftovers: the fresh files replace them.
Type: files; Name: "{app}\update-pending.json"
Type: filesandordirs; Name: "{app}\rollback"
Type: filesandordirs; Name: "{app}\update"

[Files]
Source: "{#SourceDir}\*"; Excludes: "rusty-popper.toml"; DestDir: "{app}"; Flags: ignoreversion
; Settings: only on a fresh install, and left behind on uninstall.
Source: "{#SourceDir}\rusty-popper.toml"; DestDir: "{app}"; Flags: onlyifdoesntexist uninsneveruninstall

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\Enable-RustyPopperStartup.ps1"" -ExePath ""{app}\{#AppExe}"""; WorkingDir: "{app}"; Tasks: startup; Flags: runhidden waituntilterminated; StatusMsg: "Setting Rusty Popper to start at boot..."
Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Description: "Start Rusty Popper now"; Flags: postinstall nowait skipifsilent

[UninstallDelete]
; What the app makes next to itself. rusty-popper.toml and backups\ stay.
Type: filesandordirs; Name: "{app}\cache"
Type: filesandordirs; Name: "{app}\rollback"
Type: filesandordirs; Name: "{app}\update"
Type: files; Name: "{app}\update-pending.json"
Type: files; Name: "{app}\rusty-popper.log"
Type: files; Name: "{app}\rusty-popper.old.log"

[Code]
const
  DefaultPopperDir = 'C:\vPinball\PinUPSystem';
  InstanceMutex = 'Local\RustyPopperMenu';
  StartupTask = 'Run Rusty Popper';

var
  PopperPage: TInputDirWizardPage;
  PopperDir: String;
  TomlExisted: Boolean;

function HasDb(const Dir: String): Boolean;
begin
  Result := (Dir <> '') and FileExists(AddBackslash(Dir) + 'PUPDatabase.db');
end;

{ The folder of PinUpPlayer.exe as registered with Windows (Popper's installer
  registers it as a COM server), as discover.rs does. }
function RegisteredPopperDir: String;
var
  Clsid, Server, Exe: String;
  P: Integer;
begin
  Result := '';
  if not RegQueryStringValue(HKCR, 'PinUpPlayer.PinDisplay\Clsid', '', Clsid) then
    Exit;
  if not RegQueryStringValue(HKCR, 'WOW6432Node\CLSID\' + Clsid + '\LocalServer32', '', Server) then
    if not RegQueryStringValue(HKCR, 'CLSID\' + Clsid + '\LocalServer32', '', Server) then
      Exit;
  Server := Trim(Server);
  if Copy(Server, 1, 1) = '"' then begin
    Exe := Copy(Server, 2, Length(Server));
    P := Pos('"', Exe);
    if P > 0 then
      Exe := Copy(Exe, 1, P - 1);
  end else begin
    P := Pos('.exe', Lowercase(Server));
    if P > 0 then
      Exe := Copy(Server, 1, P + 3)
    else
      Exe := Server;
  end;
  Result := ExtractFileDir(Trim(Exe));
end;

{ The usual PinUPSystem locations on every drive, then any top-level folder
  with a PinUPSystem inside it. }
function ScanDrives: String;
var
  Letter, I, Count: Integer;
  Root: String;
  Usual: array of String;
  Rec: TFindRec;
begin
  Result := '';
  SetArrayLength(Usual, 7);
  Usual[0] := 'vPinball\PinUPSystem';
  Usual[1] := 'PinUPSystem';
  Usual[2] := 'Pinball\PinUPSystem';
  Usual[3] := 'Visual Pinball\PinUPSystem';
  Usual[4] := 'PinUP\PinUPSystem';
  Usual[5] := 'Games\PinUPSystem';
  Usual[6] := 'Games\vPinball\PinUPSystem';
  for Letter := Ord('C') to Ord('Z') do begin
    Root := Chr(Letter) + ':\';
    if not DirExists(Root) then
      Continue;
    for I := 0 to GetArrayLength(Usual) - 1 do
      if HasDb(Root + Usual[I]) then begin
        Result := Root + Usual[I];
        Exit;
      end;
    if FindFirst(Root + '*', Rec) then begin
      try
        Count := 0;
        repeat
          if (Rec.Attributes and FILE_ATTRIBUTE_DIRECTORY <> 0) and HasDb(Root + Rec.Name + '\PinUPSystem') then begin
            Result := Root + Rec.Name + '\PinUPSystem';
            Exit;
          end;
          Count := Count + 1;
        until (Count >= 400) or not FindNext(Rec);
      finally
        FindClose(Rec);
      end;
    end;
  end;
end;

function DetectPopper: String;
begin
  Result := RemoveBackslashUnlessRoot(Trim(ExpandConstant('{param:POPPER|}')));
  if Result <> '' then
    Exit;
  Result := DefaultPopperDir;
  if HasDb(Result) then
    Exit;
  Result := RegisteredPopperDir;
  if HasDb(Result) then
    Exit;
  Result := ScanDrives;
end;

{ A RustyPopper folder beside PinUPSystem. }
function InstallDirFor(const Popper: String): String;
begin
  if Popper <> '' then
    Result := AddBackslash(ExtractFileDir(RemoveBackslashUnlessRoot(Popper))) + 'RustyPopper'
  else
    Result := ExpandConstant('{sd}') + '\vPinball\RustyPopper';
end;

function DefaultInstallDir(Param: String): String;
begin
  Result := InstallDirFor(PopperDir);
end;

function IsUpgrade: Boolean;
begin
  Result := (WizardForm.PrevAppDir <> '') or FileExists(AddBackslash(WizardDirValue) + '{#AppExe}');
end;

function InitializeSetup: Boolean;
begin
  PopperDir := DetectPopper;
  Result := True;
end;

procedure InitializeWizard;
var
  Intro: String;
begin
  if PopperDir <> '' then
    Intro := 'Setup found PinUP Popper in the folder below. Change it if that''s not the one.'
  else
    Intro := 'Setup couldn''t find PinUP Popper. Pick its PinUPSystem folder (the one holding PUPDatabase.db).';
  PopperPage := CreateInputDirPage(wpWelcome, 'PinUP Popper', 'Where is PinUP Popper installed?',
    Intro + #13#10#13#10 + 'Rusty Popper uses Popper''s games, media and controller setup, and installs into a RustyPopper folder beside it.',
    False, '');
  PopperPage.Add('PinUPSystem folder:');
  PopperPage.Values[0] := PopperDir;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  { An upgrade of this setup's install already knows where everything is. }
  Result := (PageID = PopperPage.ID) and (WizardForm.PrevAppDir <> '');
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Dir: String;
begin
  Result := True;
  if CurPageID = PopperPage.ID then begin
    Dir := RemoveBackslashUnlessRoot(Trim(PopperPage.Values[0]));
    if not HasDb(Dir) then
      if SuppressibleMsgBox('There''s no PUPDatabase.db in "' + Dir + '", so this doesn''t look like PinUP Popper''s PinUPSystem folder.' +
           #13#10#13#10 + 'Continue anyway?', mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDYES) <> IDYES then begin
        Result := False;
        Exit;
      end;
    if CompareText(Dir, PopperDir) <> 0 then begin
      PopperDir := Dir;
      WizardForm.DirEdit.Text := InstallDirFor(PopperDir);
    end;
  end;
end;

function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo,
  MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
begin
  Result := MemoDirInfo + NewLine;
  if WizardForm.PrevAppDir = '' then
    Result := Result + NewLine + 'PinUP Popper:' + NewLine + Space + PopperDir + NewLine;
  if IsUpgrade then
    Result := Result + NewLine + 'Upgrade:' + NewLine + Space +
      'The program files are replaced; your rusty-popper.toml settings are kept.' + NewLine;
  if MemoTasksInfo <> '' then
    Result := Result + NewLine + MemoTasksInfo + NewLine;
end;

procedure CloseRustyPopper;
var
  Code, I: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
  { Wait for the files to be let go. }
  for I := 1 to 20 do begin
    if not CheckForMutexes(InstanceMutex) then
      Break;
    Sleep(250);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if CheckForMutexes(InstanceMutex) then begin
    if SuppressibleMsgBox('Rusty Popper is running. Setup will close it (and any game it started keeps running).',
         mbInformation, MB_OKCANCEL, IDOK) <> IDOK then begin
      Result := 'Rusty Popper is still running. Close it and run Setup again.';
      Exit;
    end;
    CloseRustyPopper;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Toml, Text: String;
  Existing: AnsiString;
begin
  Toml := ExpandConstant('{app}\rusty-popper.toml');
  if CurStep = ssInstall then
    TomlExisted := FileExists(Toml)
  else if CurStep = ssPostInstall then begin
    { A fresh install: save the Popper folder in the settings unless the app
      would find it straight away anyway. }
    if TomlExisted or (PopperDir = '') or (CompareText(PopperDir, DefaultPopperDir) = 0) then
      Exit;
    if LoadStringFromFile(Toml, Existing) and (Pos('[paths]', String(Existing)) > 0) then
      Exit;
    Text := #13#10 + '[paths]' + #13#10 + 'popper_dir = ''' + PopperDir + '''' + #13#10;
    SaveStringToFile(Toml, Text, True);
  end;
end;

{ True when the "Run Rusty Popper" logon task exists and is enabled. }
function StartupTaskEnabled: Boolean;
var
  Code: Integer;
begin
  Result := Exec('powershell.exe',
    '-NoProfile -Command "if ((Get-ScheduledTask -TaskName ''' + StartupTask + ''' -ErrorAction SilentlyContinue).State -in ''Ready'',''Running'') { exit 0 } else { exit 1 }"',
    '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
  Script: String;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  if CheckForMutexes(InstanceMutex) then
    CloseRustyPopper;
  { Don't leave the cabinet starting a front end that's gone: hand boot back
    to Popper (the script asks for administrator rights). }
  Script := ExpandConstant('{app}\Disable-RustyPopperStartup.ps1');
  if FileExists(Script) and StartupTaskEnabled then
    Exec('powershell.exe', '-NoProfile -ExecutionPolicy Bypass -File "' + Script + '"', ExpandConstant('{app}'),
      SW_HIDE, ewWaitUntilTerminated, Code);
end;
