; Glide-Dock installer (Inno Setup 6)
;
; Build with:
;   ISCC.exe /DStageDir="<package stage>" /DVersion="0.4.0" installer\Glide-Dock.iss
;
; The stage is produced by scripts/package.ps1.

#define AppName "Glide-Dock"
#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{9C6F1D7A-4B23-4D8E-9C1A-6E2F0A7B3C51}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher=Glide-Dock contributors
AppPublisherURL=https://github.com/Glide-Dock/Glide-Dock
DefaultDirName={autopf}\Glide-Dock
DefaultGroupName=Glide-Dock
DisableProgramGroupPage=yes
DisableWelcomePage=no
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes
WizardStyle=modern
OutputDir={#OutputDir}
OutputBaseFilename=Glide-Dock-v{#Version}-Setup
SetupIconFile=..\assets\brand\03-Windows-ICO\Glide-Dock-FullColor.ico
UninstallDisplayIcon={app}\glide-dock.exe
UninstallDisplayName={#AppName} {#Version}
SetupLogging=yes
UsePreviousAppDir=yes
CloseApplications=yes
RestartApplications=no
VersionInfoVersion={#Version}.0
VersionInfoCompany=Glide-Dock contributors
VersionInfoDescription=Glide-Dock installer
VersionInfoProductName={#AppName}
VersionInfoProductVersion={#Version}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\glide-dock.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\glide-dock.exe"; Tasks: desktopicon

[InstallDelete]
Type: files; Name: "{group}\{#AppName} 设置.lnk"
Type: files; Name: "{autodesktop}\{#AppName} 设置.lnk"

[UninstallDelete]
Type: files; Name: "{group}\{#AppName} 设置.lnk"
Type: files; Name: "{autodesktop}\{#AppName} 设置.lnk"

[Run]
Filename: "{app}\glide-dock.exe"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM GlideDock.Settings.exe /F"; Flags: runhidden; RunOnceId: "StopSettings"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM glide-dock.exe /F"; Flags: runhidden; RunOnceId: "StopDock"
Filename: "{sys}\schtasks.exe"; Parameters: "/Delete /TN ""Glide-Dock"" /F"; Flags: runhidden; RunOnceId: "RemoveStartupTask"
Filename: "{sys}\reg.exe"; Parameters: "delete ""HKCU\Software\Microsoft\Windows\CurrentVersion\Run"" /v ""Glide-Dock"" /f"; Flags: runhidden; RunOnceId: "RemoveRunValue"

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  // The hidden guard can restart the dock while Setup is replacing files.
  // Stop both processes before the install copy; the uninstaller does the
  // same through [UninstallRun].
  if CurStep = ssInstall then
  begin
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM GlideDock.Settings.exe /F',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM glide-dock.exe /F',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end;
end;
