[Setup]
AppId={{37C7B223-B8DD-4F5E-9932-6FB1B6E63324}
AppName=ClipVault
AppVersion=0.1.0
DefaultDirName={localappdata}\Programs\ClipVault
DefaultGroupName=ClipVault
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=ClipVault-Windows-Setup
Compression=lzma2
SolidCompression=yes
UninstallDisplayIcon={app}\clipvault.exe
[Files]
Source: "..\target\release\clipvault.exe"; DestDir: "{app}"
Source: "..\README.md"; DestDir: "{app}"
Source: "..\LICENSE"; DestDir: "{app}"
[Icons]
Name: "{group}\ClipVault"; Filename: "{app}\clipvault.exe"
[Run]
Filename: "{app}\clipvault.exe"; Description: "Start ClipVault"; Flags: nowait postinstall skipifsilent
