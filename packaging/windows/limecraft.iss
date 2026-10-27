; LimeCraft installer for Windows (Inno Setup 6.4 or newer: https://jrsoftware.org/isdl.php).
; Build it with packaging\windows\build-installer.ps1, or by hand:
;   ISCC.exe /DAppVersion=1.0.0 /DSourceExe=..\..\target\release\limecraft-launcher.exe limecraft.iss
; The result goes to dist\ in the project root.
;
; By default it installs for the current user only (no admin rights needed, into
; %LOCALAPPDATA%\Programs\LimeCraft); the first page lets the user pick "all users"
; instead. Games and settings live in %APPDATA%\LimeCraft and survive updates;
; the uninstaller asks whether to delete them.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=1.2.3 limecraft.iss
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\limecraft-launcher.exe"
#endif
#define AppName "LimeCraft"
#define AppExe "LimeCraft.exe"
#define Root "..\.."

[Setup]
; never change AppId: updates and the uninstaller find the installed copy by it
AppId={{6F1C2B7E-4A8D-4E5B-9C3F-1D2E7A9B4C60}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=LimeCraft
AppComments=Minecraft launcher: mods, modpacks, offline skins
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#Root}\dist
OutputBaseFilename=LimeCraft-{#AppVersion}-setup-x64
SetupIconFile={#Root}\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern dynamic
WizardImageFile=wizard-100.png,wizard-150.png,wizard-200.png
WizardSmallImageFile=wizard-small-100.png,wizard-small-150.png,wizard-small-200.png
WizardImageStretch=no
WizardImageBackColor=$1d201b
Compression=lzma2/ultra64
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
ShowLanguageDialog=auto
LanguageDetectionMethod=uilanguage

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Messages]
ru.WelcomeLabel2=Сейчас на ваш компьютер будет установлен [name/ver].%n%nЛаунчер сам скачает нужную версию Minecraft и Java. Моды, сборки с Modrinth и офлайн-скины — в одном окне.%n%nНажмите «Далее», чтобы продолжить.
en.WelcomeLabel2=This will install [name/ver] on your computer.%n%nThe launcher downloads the right Minecraft version and Java by itself. Mods, Modrinth modpacks and offline skins, all in one window.%n%nClick Next to continue.
ru.FinishedLabel=Готово! [name] установлен.%n%nЗапустите его ярлыком в меню «Пуск» или на рабочем столе. Приятной игры!
en.FinishedLabel=Done! [name] is installed.%n%nStart it from the Start menu or the desktop shortcut. Have fun!

[CustomMessages]
ru.ShortcutsGroup=Ярлыки:
en.ShortcutsGroup=Shortcuts:
ru.DesktopShortcut=Ярлык на рабочем столе
en.DesktopShortcut=Desktop shortcut
ru.LaunchNow=Запустить LimeCraft
en.LaunchNow=Start LimeCraft
ru.DeleteData=Удалить также ваши игры, миры и настройки?%n%n%1%n%nЭто нельзя отменить. Нажмите «Нет», чтобы сохранить их на случай повторной установки.
en.DeleteData=Also delete your games, worlds and settings?%n%n%1%n%nThis cannot be undone. Click No to keep them in case you install LimeCraft again.

[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopShortcut}"; GroupDescription: "{cm:ShortcutsGroup}"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#Root}\assets\fonts\PHOSPHOR-LICENSE-MIT.txt"; DestDir: "{app}\licenses"; DestName: "Phosphor-Icons-MIT.txt"; Flags: ignoreversion
Source: "{#Root}\assets\fonts\Manrope-OFL.txt"; DestDir: "{app}\licenses"; DestName: "Manrope-OFL.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Minecraft launcher"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Minecraft launcher"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchNow}"; Flags: nowait postinstall skipifsilent

[Code]
function DataDir(): String;
begin
  Result := GetEnv('LIMECRAFT_HOME');
  if Result = '' then
    Result := ExpandConstant('{userappdata}\LimeCraft');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Dir: String;
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent() then
  begin
    Dir := DataDir();
    if DirExists(Dir) and
       (MsgBox(FmtMessage(CustomMessage('DeleteData'), [Dir]), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
      DelTree(Dir, True, True, True);
  end;
end;
