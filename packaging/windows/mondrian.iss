; Installer für Mondrian (Inno Setup 6). Wird von build.sh aufgerufen, das die Werte
; AppVersion, SourceDir (fertiger Programmordner) und IconFile übergibt.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\windows\Mondrian"
#endif
#ifndef IconFile
  #define IconFile "..\..\data\mondrian.ico"
#endif

[Setup]
; feste Kennung – bei Updates gleich lassen, damit ältere Versionen ersetzt werden
AppId={{6F3D2A71-8C4B-4E59-A1D7-2B9E5C0F4A13}
AppName=Mondrian
AppVersion={#AppVersion}
AppVerName=Mondrian {#AppVersion}
AppPublisher=Mondrian
AppComments=Vektorzeichenprogramm (Testversion)
DefaultDirName={autopf}\Mondrian
DefaultGroupName=Mondrian
DisableProgramGroupPage=yes
LicenseFile={#SourceDir}\LICENSE.txt
OutputBaseFilename=Mondrian-{#AppVersion}-Setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\bin\mondrian.exe
UninstallDisplayName=Mondrian
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; wahlweise für alle Benutzer (Administrator) oder nur für mich (ohne Administrator)
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes
; ein laufendes Mondrian vor dem Aktualisieren schließen
CloseApplications=yes

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; Reste einer älteren Version (z. B. nicht mehr benötigte DLLs) entfernen
Type: filesandordirs; Name: "{app}\bin"
Type: filesandordirs; Name: "{app}\lib"
Type: filesandordirs; Name: "{app}\share"

[Icons]
Name: "{autoprograms}\Mondrian"; Filename: "{app}\bin\mondrian.exe"; IconFilename: "{app}\mondrian.ico"
Name: "{autodesktop}\Mondrian"; Filename: "{app}\bin\mondrian.exe"; IconFilename: "{app}\mondrian.ico"; Tasks: desktopicon

[Registry]
; Dateityp „Mondrian-Grafik“ für „Öffnen mit“ (die Standard-App für SVG bleibt unverändert)
Root: HKA; Subkey: "Software\Classes\Mondrian.Document"; ValueType: string; ValueName: ""; ValueData: "Grafik"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Mondrian.Document\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\mondrian.ico"
Root: HKA; Subkey: "Software\Classes\Mondrian.Document\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\bin\mondrian.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.svg\OpenWithProgids"; ValueType: string; ValueName: "Mondrian.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ai\OpenWithProgids"; ValueType: string; ValueName: "Mondrian.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Mondrian.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\mondrian.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Mondrian"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\mondrian.exe\SupportedTypes"; ValueType: string; ValueName: ".svg"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\mondrian.exe\SupportedTypes"; ValueType: string; ValueName: ".ai"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\mondrian.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\mondrian.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\bin\mondrian.exe"" ""%1"""

[Run]
Filename: "{app}\bin\mondrian.exe"; Description: "{cm:LaunchProgram,Mondrian}"; Flags: nowait postinstall skipifsilent
