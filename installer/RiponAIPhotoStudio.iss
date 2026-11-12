; Inno Setup script - Ripon AI Photo Studio  (https://jrsoftware.org/isinfo.php)
; Build:  ISCC.exe installer\RiponAIPhotoStudio.iss   (after cargo build --release)
#define AppName "rust.riponaistudio"
#define ShortName "Ripon AI Studio (Rust)"
#define AppVersion "1.4.3"
#define AppExe "rust.riponaistudio.exe"

[Setup]
AppId={{B7C1F3E2-5A64-4D1B-9C0E-52A8F1D4A001}
AppName={#AppName}
AppVerName={#AppName} {#AppVersion}
AppVersion={#AppVersion}
AppPublisher=Ripon Computer
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile=..\assets\ripon_logo.ico
OutputDir=..\dist
OutputBaseFilename=rust.riponaistudio-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
WizardStyle=modern
DisableProgramGroupPage=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "..\target\release\ripon-ai-photo-studio.exe"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
; the dress folder lives next to the AI models, same root as the Python app
Source: "..\dress\README.txt"; DestDir: "{localappdata}\RiponComputer\dress"; Flags: onlyifdoesntexist uninsneveruninstall

[Dirs]
Name: "{localappdata}\RiponComputer\models\rembg"; Flags: uninsneveruninstall
Name: "{localappdata}\RiponComputer\models\gfpgan"; Flags: uninsneveruninstall
Name: "{localappdata}\RiponComputer\models\realesrgan"; Flags: uninsneveruninstall
Name: "{localappdata}\RiponComputer\models\yunet"; Flags: uninsneveruninstall
Name: "{localappdata}\RiponComputer\models\document"; Flags: uninsneveruninstall

[Icons]
Name: "{autoprograms}\{#ShortName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#ShortName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start {#ShortName}"; Flags: nowait postinstall skipifsilent

; AI models and settings in %LOCALAPPDATA%\RiponComputer are NOT removed on uninstall (they are large).
