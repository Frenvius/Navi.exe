#ifndef AppVersion
  #error Pass the version with /DAppVersion=x.y.z
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\navi.exe"
#endif

[Setup]
AppId={{7C1E4B52-9A3D-4F6E-8B21-5D0A9E3C6F14}
AppName=Navi
AppVersion={#AppVersion}
AppVerName=Navi {#AppVersion}
AppPublisher=Frenvius
AppPublisherURL=https://github.com/Frenvius/Navi.exe
AppSupportURL=https://github.com/Frenvius/Navi.exe/issues
AppUpdatesURL=https://github.com/Frenvius/Navi.exe/releases
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Navi
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
LicenseFile=..\LICENSE
SetupIconFile=..\ui\assets\explorer.ico
UninstallDisplayIcon={app}\navi.exe
UninstallDisplayName=Navi
OutputDir=..\dist
OutputBaseFilename=Navi-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Navi"; Filename: "{app}\navi.exe"; AppUserModelID: "Navi.App"
Name: "{autodesktop}\Navi"; Filename: "{app}\navi.exe"; AppUserModelID: "Navi.App"; Tasks: desktopicon

[Run]
Filename: "{app}\navi.exe"; Description: "{cm:LaunchProgram,Navi}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\navi.exe"; Flags: nowait; Check: IsRelaunch

[UninstallRun]
Filename: "{app}\navi.exe"; Parameters: "--unregister"; Flags: runhidden waituntilterminated; RunOnceId: "Unregister"

[Code]
function IsRelaunch: Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
      Result := True;
end;
