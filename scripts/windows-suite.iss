; One offline suite. Never download tools, elevate, or change firewall rules.
[Setup]
AppId={{DFEC1BDA-2C8E-4499-A51C-66CC73D83279}
AppName=ROS Studio
AppVersion={#SuiteVersion}
AppPublisher=Netizen Robotics
AppPublisherURL=https://github.com/netizen-robotics/ros-studio
VersionInfoProductName=ROS Studio
VersionInfoVersion={#SuiteVersion}
DefaultDirName={localappdata}\Programs\ROS Studio
DefaultGroupName=ROS Studio
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
SourceDir={#SourceDir}
OutputDir={#OutputDir}
OutputBaseFilename=ros-studio-{#SuiteVersion}-windows-x64-setup
SetupIconFile={#SuiteIcon}
UninstallDisplayIcon={app}\studio-launcher.exe
LicenseFile=LICENSE
InfoBeforeFile=THIRD_PARTY_NOTICES.md
Compression=lzma2/fast
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
Uninstallable=yes
ChangesEnvironment=no

[Files]
Source: "*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\ROS Studio"; Filename: "{app}\studio-launcher.exe"; WorkingDir: "{app}"
Name: "{group}\Uninstall ROS Studio"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\studio-launcher.exe"; Description: "Open ROS Studio"; Flags: nowait postinstall skipifsilent unchecked
