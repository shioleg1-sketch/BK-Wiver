#define MyAppName "BK-Console"
#define MyAppVersion "0.1.30"
#define MyAppPublisher "BK-Wiver"
#define MyAppExeName "bk-wiver-console.exe"
#define MyAppSource ".\stage\bk-wiver-console.exe"
#define MyAppIcon "..\..\assets\app-icon.ico"

[Setup]
AppId={{7BE0B1AE-AE92-4F9F-8BA8-F00565BE907C}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={commonpf}\BK-Console
DefaultGroupName=BK-Console
DisableProgramGroupPage=yes
OutputDir=..\..\..\dist
OutputBaseFilename=BK-Console-Setup
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
Name: "{commonprograms}\BK-Console"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\app-icon.ico"
Name: "{commondesktop}\BK-Console"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\app-icon.ico"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch BK-Console"; Flags: nowait postinstall skipifsilent
