; RescueForge installer (Inno Setup 6).

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
; Where cargo put the build ("target" in the project unless CARGO_TARGET_DIR says otherwise).
#ifndef TargetDir
  #define TargetDir "..\target"
#endif

[Setup]
AppId={{7C4E2A91-5B38-4D6F-A2C7-91E0D3B6F548}
AppName=RescueForge
AppVersion={#AppVersion}
AppVerName=RescueForge {#AppVersion}
AppPublisher=Zheer Khalid
AppPublisherURL=https://zheer.dev
AppSupportURL=https://github.com/zheerkhalid/rescueforge
AppUpdatesURL=https://github.com/zheerkhalid/rescueforge/releases
DefaultDirName={autopf}\RescueForge
DefaultGroupName=RescueForge
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible 
ArchitecturesInstallIn64BitMode=x64compatible 
OutputDir=..\dist
OutputBaseFilename=RescueForge-Setup
SetupIconFile={#TargetDir}\rescueforge.ico
UninstallDisplayIcon={app}\RescueForge.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
LicenseFile=..\LICENSE
InfoBeforeFile=before-install.txt
VersionInfoVersion={#AppVersion}
VersionInfoCompany=Zheer Khalid (zheer.dev)
VersionInfoDescription=RescueForge Setup

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; 

[Files]
Source: "{#TargetDir}\release\RescueForge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\NOTICE"; DestDir: "{app}"; DestName: "NOTICE.txt"; Flags: ignoreversion
Source: "..\docs\HOW-IT-WORKS.md"; DestDir: "{app}"; DestName: "HOW-IT-WORKS.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\RescueForge"; Filename: "{app}\RescueForge.exe"
Name: "{group}\Uninstall RescueForge"; Filename: "{uninstallexe}"
Name: "{autodesktop}\RescueForge"; Filename: "{app}\RescueForge.exe"; Tasks: desktopicon


[Run]
Filename: "{app}\RescueForge.exe"; Description: "{cm:LaunchProgram,RescueForge}"; Flags: nowait postinstall skipifsilent shellexec

[UninstallRun]
Filename: "{cmd}"; Parameters: "/C taskkill /IM RescueForge.exe /F"; Flags: runhidden; RunOnceId: "KillRescueForge"
