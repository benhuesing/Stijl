; SPDX-License-Identifier: GPL-3.0-or-later
; Copyright © 2026 Bendix Hüsing
;
; Installer für Stijl (Inno Setup 6). Wird von build.sh aufgerufen, das die Werte
; AppVersion, SourceDir (fertiger Programmordner) und IconFile übergibt.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\windows\Stijl"
#endif
#ifndef IconFile
  #define IconFile "..\..\data\stijl.ico"
#endif

; Kennung der Testversionen, die noch „Mondrian“ hießen (werden beim Installieren entfernt)
#define OldAppId "{6F3D2A71-8C4B-4E59-A1D7-2B9E5C0F4A13}"

[Setup]
; feste Kennung – bei Updates gleich lassen, damit ältere Versionen ersetzt werden
AppId={{2CFC1B4F-3821-41A1-9883-66AC1AE42E41}
AppName=Stijl
AppVersion={#AppVersion}
AppVerName=Stijl {#AppVersion}
AppPublisher=Bendix Hüsing
AppPublisherURL=https://github.com/benhuesing/Stijl
AppSupportURL=https://github.com/benhuesing/Stijl/issues
AppUpdatesURL=https://github.com/benhuesing/Stijl/releases
AppCopyright=Copyright © 2026 Bendix Hüsing
AppComments=Vektorzeichenprogramm (Testversion)
VersionInfoVersion={#AppVersion}
VersionInfoCompany=Bendix Hüsing
VersionInfoCopyright=Copyright © 2026 Bendix Hüsing
DefaultDirName={autopf}\Stijl
DefaultGroupName=Stijl
DisableProgramGroupPage=yes
LicenseFile={#SourceDir}\LICENSE.txt
OutputBaseFilename=Stijl-{#AppVersion}-Setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\bin\stijl.exe
UninstallDisplayName=Stijl
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; wahlweise für alle Benutzer (Administrator) oder nur für mich (ohne Administrator)
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes
; ein laufendes Stijl vor dem Aktualisieren schließen
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
Name: "{autoprograms}\Stijl"; Filename: "{app}\bin\stijl.exe"; IconFilename: "{app}\stijl.ico"
Name: "{autodesktop}\Stijl"; Filename: "{app}\bin\stijl.exe"; IconFilename: "{app}\stijl.ico"; Tasks: desktopicon

[Registry]
; Dateityp „Stijl-Grafik“ für „Öffnen mit“ (die Standard-App für SVG bleibt unverändert)
Root: HKA; Subkey: "Software\Classes\Stijl.Document"; ValueType: string; ValueName: ""; ValueData: "Grafik"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Stijl.Document\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\stijl.ico"
Root: HKA; Subkey: "Software\Classes\Stijl.Document\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\bin\stijl.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.svg\OpenWithProgids"; ValueType: string; ValueName: "Stijl.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ai\OpenWithProgids"; ValueType: string; ValueName: "Stijl.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Stijl.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\stijl.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Stijl"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\stijl.exe\SupportedTypes"; ValueType: string; ValueName: ".svg"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\stijl.exe\SupportedTypes"; ValueType: string; ValueName: ".ai"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\stijl.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\stijl.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\bin\stijl.exe"" ""%1"""

[Run]
Filename: "{app}\bin\stijl.exe"; Description: "{cm:LaunchProgram,Stijl}"; Flags: nowait postinstall skipifsilent

[Code]
// Eine installierte Testversion unter dem alten Namen „Mondrian“ still deinstallieren
// (Einstellungen und Bibliotheken bleiben erhalten; Stijl übernimmt sie beim ersten Start).
function OldUninstaller(): String;
var
  Key: String;
begin
  Result := '';
  Key := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#OldAppId}_is1';
  if not RegQueryStringValue(HKLM, Key, 'UninstallString', Result) then
    RegQueryStringValue(HKCU, Key, 'UninstallString', Result);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Cmd: String;
  Code: Integer;
begin
  if CurStep = ssInstall then
  begin
    Cmd := RemoveQuotes(OldUninstaller());
    if (Cmd <> '') and FileExists(Cmd) then
      Exec(Cmd, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, Code);
  end;
end;
