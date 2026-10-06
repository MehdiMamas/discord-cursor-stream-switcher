; Inno Setup script for Cursor Stream Switcher (Inno Setup 6.3 or later).
;
;   cargo build --release
;   iscc installer\cursor-stream-switcher.iss      -> dist\cursor-stream-switcher-setup.exe
;
; The version is read from the built .exe (Cargo.toml), so it can't drift.
;
; Installs per machine into Program Files and registers in Settings > Apps. The app updates by
; downloading the next release of this installer and running it with /SILENT /relaunch=1
; (src/update.rs). Uninstalling also removes the virtual display driver and the autostart entry.

#define AppName "Cursor Stream Switcher"
#define AppExe "cursor-stream-switcher.exe"
#define AppSource "..\target\release\" + AppExe
#define RepoUrl "https://github.com/MehdiMamas/discord-cursor-stream-switcher"
#if !FileExists(AppSource)
  #error Build the app first: cargo build --release
#endif
#define AppVersion GetFileProductVersion(AppSource)

[Setup]
; Never change AppId: Windows finds the existing install by it, for upgrades and uninstall.
AppId={{E8DC6E4A-4010-47D6-BF45-64ECC95B33B5}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=MehdiMamas
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
AppCopyright=MIT License
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=cursor-stream-switcher-setup
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; [Code] asks the app to quit first; this force-closes a copy that doesn't (e.g. in another
; user's session) so a silent update never stalls. The app is restarted by [Run] instead.
CloseApplications=force
RestartApplications=no
; The autostart value goes to HKCU of whoever runs Setup: the signed-in user in practice.
UsedUserAreasWarning=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start {#AppName} with Windows"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#AppSource}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "..\vendor\vdd\LICENSE"; DestDir: "{app}"; DestName: "VirtualDisplayDriver-LICENSE.txt"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The same value the tray menu's "Start with Windows" writes (src/system.rs). Not rewritten
; on updates, so turning it off in the tray menu sticks.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "CursorStreamSwitcher"; ValueData: """{app}\{#AppExe}"""; Tasks: autostart; Check: not IsRelaunch

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: postinstall nowait skipifsilent
; Updates started from the app: start the new version as the signed-in user, not as admin.
Filename: "{app}\{#AppExe}"; Parameters: "--after-update"; Flags: nowait runasoriginaluser; Check: IsRelaunch

[UninstallDelete]
; Left by the driver commands (src/paths.rs machine_dir); the driver removal empties it.
Type: dirifempty; Name: "{commonappdata}\CursorStreamSwitcher"

[Code]
const
  TrayWindowClass = 'CursorStreamSwitcher.Tray';    { src/tray.rs }
  AppMutex = 'Local\CursorStreamSwitcher.SingleInstance';  { src/main.rs }
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  RunValue = 'CursorStreamSwitcher';                { src/system.rs }
  DataDir = 'CursorStreamSwitcher';                 { src/paths.rs APP_DIR }
  DriverExitReboot = 2;                             { src/driver.rs EXIT_REBOOT }
  WM_CLOSE = $0010;

var
  RestartNeeded: Boolean;

function IsRelaunch: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;

{ Asks a running copy to quit (WM_CLOSE does the same as tray menu > Quit) and waits up to
  10 seconds for it to exit. }
procedure CloseRunningApp;
var
  Wnd: HWND;
  I: Integer;
begin
  for I := 1 to 50 do
  begin
    Wnd := FindWindowByClassName(TrayWindowClass);
    if Wnd <> 0 then
      PostMessage(Wnd, WM_CLOSE, 0, 0)
    else if not CheckForMutexes(AppMutex) then
    begin
      if I > 1 then
        Sleep(500); { the mutex is released just before the process ends }
      Exit;
    end;
    Sleep(200);
  end;
  Log('The app did not close within 10 seconds');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseRunningApp;
  Result := '';
end;

{ The driver commands need admin rights, which the uninstaller has. }
procedure RemoveDriver;
var
  Code: Integer;
begin
  if not Exec(ExpandConstant('{app}\{#AppExe}'), 'driver-uninstall', '', SW_HIDE,
    ewWaitUntilTerminated, Code) then
    Code := -1;
  Log(Format('driver-uninstall exited with %d', [Code]));
  if Code = DriverExitReboot then
    RestartNeeded := True
  else if Code <> 0 then
    SuppressibleMsgBox('The virtual display driver could not be removed. Details are in '
      + ExpandConstant('{localappdata}\' + DataDir + '\driver.log') + '.' + #13#10#13#10
      + 'You can remove it in Device Manager > Display adapters > Virtual Display Driver.',
      mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  case CurUninstallStep of
    usUninstall:
      begin
        CloseRunningApp;
        RemoveDriver;
        RegDeleteValue(HKCU, RunKey, RunValue);
      end;
    usPostUninstall:
      if not UninstallSilent and (MsgBox('Also delete your ' + '{#AppName}' + ' settings and logs?',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
      begin
        DelTree(ExpandConstant('{userappdata}\' + DataDir), True, True, True);
        DelTree(ExpandConstant('{localappdata}\' + DataDir), True, True, True);
      end;
  end;
end;

function UninstallNeedRestart: Boolean;
begin
  Result := RestartNeeded;
end;
