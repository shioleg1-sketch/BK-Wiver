#define MyAppName "BK-Host"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "BK-Wiver"
#define MyAppExeName "bk-wiver-host.exe"
#define MyAppSource "..\..\..\target\release\bk-wiver-host.exe"
#define MyAppIcon "..\..\assets\app-icon.ico"
#define MyServiceName "BKWiverHostService"
#define MyServiceDisplayName "BK-Host Service"
#define MyAgentTaskName "BK-Host Agent"

[Setup]
AppId={{CA45CC7A-EBD5-4B7F-B469-48E9E63A0670}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={commonpf}\BK-Host
DefaultGroupName=BK-Host
DisableProgramGroupPage=yes
OutputDir=..\..\..\dist
OutputBaseFilename=BK-Host-Setup
SetupIconFile={#MyAppIcon}
Compression=lzma
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Files]
Source: "{#MyAppSource}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#MyAppIcon}"; DestDir: "{app}"; DestName: "app-icon.ico"; Flags: ignoreversion

[Icons]
Name: "{commonprograms}\BK-Host"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\app-icon.ico"
Name: "{commondesktop}\BK-Host"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\app-icon.ico"
Name: "{commonstartup}\BK-Host"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\app-icon.ico"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Запустить BK-Host"; Flags: nowait postinstall skipifsilent

[Code]
procedure ExecAndLog(const FileName, Params: string; const WorkingDir: string);
var
  ResultCode: Integer;
begin
  if not Exec(FileName, Params, WorkingDir, SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Log(Format('Exec failed: %s %s', [FileName, Params]))
  else
    Log(Format('Exec finished (%d): %s %s', [ResultCode, FileName, Params]));
end;

procedure RegisterHostRuntime;
var
  HostExe: string;
begin
  HostExe := ExpandConstant('{app}\{#MyAppExeName}');

  ExecAndLog(
    ExpandConstant('{sys}\sc.exe'),
    'create {#MyServiceName} binPath= ""' + HostExe + '"" --service start= auto obj= LocalSystem DisplayName= ""{#MyServiceDisplayName}""',
    ExpandConstant('{app}')
  );
  ExecAndLog(
    ExpandConstant('{sys}\sc.exe'),
    'description {#MyServiceName} "Фоновый сервис BK-Host"',
    ExpandConstant('{app}')
  );
  ExecAndLog(
    ExpandConstant('{sys}\schtasks.exe'),
    '/Create /TN "{#MyAgentTaskName}" /TR ""' + HostExe + '"" --agent /SC ONLOGON /RL LIMITED /F',
    ExpandConstant('{app}')
  );
  ExecAndLog(
    ExpandConstant('{sys}\sc.exe'),
    'start {#MyServiceName}',
    ExpandConstant('{app}')
  );
  ExecAndLog(
    ExpandConstant('{sys}\schtasks.exe'),
    '/Run /TN "{#MyAgentTaskName}"',
    ExpandConstant('{app}')
  );
end;

procedure UnregisterHostRuntime;
begin
  ExecAndLog(
    ExpandConstant('{sys}\sc.exe'),
    'stop {#MyServiceName}',
    ExpandConstant('{app}')
  );
  ExecAndLog(
    ExpandConstant('{sys}\sc.exe'),
    'delete {#MyServiceName}',
    ExpandConstant('{app}')
  );
  ExecAndLog(
    ExpandConstant('{sys}\schtasks.exe'),
    '/Delete /TN "{#MyAgentTaskName}" /F',
    ExpandConstant('{app}')
  );
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    RegisterHostRuntime;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    UnregisterHostRuntime;
end;
