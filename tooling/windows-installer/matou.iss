[Setup]
AppId=com.matou.desktop
AppName=码头
AppVersion={#MatouVersion}
AppPublisher=Matou
DefaultDirName={localappdata}\Programs\Matou
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableStartupPrompt=yes
PrivilegesRequired=lowest
SetupArchitecture=x64
#ifdef MatouArm64
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
VersionInfoProductName=Matou ARM64
#define MatouPayloadMachine 43620
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
VersionInfoProductName=Matou x64
#define MatouPayloadMachine 34404
#endif
SetupMutex=com.matou.desktop.Setup
MinVersion=10.0
UninstallDisplayIcon={app}\码头.exe
VersionInfoVersion={#MatouFileVersion}
VersionInfoProductTextVersion={#MatouVersion}
Compression=lzma2
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes
WizardStyle=modern dynamic
WizardSizePercent=105,100
SetupIconFile=..\..\apps\native\packaging\icon.ico
WizardSmallImageFile=..\..\apps\native\packaging\icon-128.png
ShowLanguageDialog=no
#ifdef MatouSigned
SignTool=matou_release_signing
SignedUninstaller=yes
SignedUninstallerDir={#MatouUninstallerDir}
#endif

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Messages]
chinesesimplified.ReadyLabel1=码头将安装到当前 Windows 用户。
english.ReadyLabel1=Matou will be installed for the current Windows user.
chinesesimplified.ReadyLabel2a=安装完成后，可从开始菜单打开码头。
english.ReadyLabel2a=After setup, open Matou from the Start menu.
chinesesimplified.ReadyLabel2b=安装完成后，可从开始菜单打开码头。
english.ReadyLabel2b=After setup, open Matou from the Start menu.

[CustomMessages]
chinesesimplified.MatouInstallLocation=安装位置
english.MatouInstallLocation=Installation location
chinesesimplified.MatouOtherArchitecture=已有的码头安装与此安装包架构不同，或无法核对架构。请使用原架构的安装包更新。若要切换架构，请先从 Windows 设置卸载码头。工作区和会话数据会保留。
english.MatouOtherArchitecture=Setup could not confirm that this installer matches the existing Matou architecture. Use the installer for the current architecture to update. To switch architectures, uninstall Matou in Windows Settings first. Your workspaces and sessions will be kept.

[Files]
#include MatouPayloadFiles

[Icons]
Name: "{userprograms}\码头"; Filename: "{app}\码头.exe"; WorkingDir: "{app}"; IconFilename: "{app}\resources\matou.ico"; AppUserModelID: "com.matou.desktop"; AppUserModelToastActivatorCLSID: "A94C9F25-457F-4DC8-A373-937E157F8262"

[Registry]
Root: HKCU; Subkey: "Software\Classes\matou-notification"; ValueType: string; ValueData: "URL:Matou Notification"
Root: HKCU; Subkey: "Software\Classes\matou-notification"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\matou-notification\shell\open\command"; ValueType: string; ValueData: """{app}\码头.exe"" ""%1"""

[Run]
Filename: "{app}\码头.exe"; WorkingDir: "{app}"; Description: "{cm:LaunchProgram,码头}"; Flags: nowait postinstall skipifsilent

[Code]
var
  InstallLocation: TNewStaticText;

procedure InitializeWizard;
var
  LocationLabel: TNewStaticText;
begin
  WizardForm.ReadyMemo.Visible := False;
  LocationLabel := TNewStaticText.Create(WizardForm);
  LocationLabel.Parent := WizardForm.ReadyMemo.Parent;
  LocationLabel.Left := WizardForm.ReadyMemo.Left;
  LocationLabel.Top := WizardForm.ReadyMemo.Top + ScaleY(8);
  LocationLabel.Caption := CustomMessage('MatouInstallLocation');
  LocationLabel.Font.Style := [fsBold];
  InstallLocation := TNewStaticText.Create(WizardForm);
  InstallLocation.Parent := LocationLabel.Parent;
  InstallLocation.Left := LocationLabel.Left;
  InstallLocation.Top := LocationLabel.Top + ScaleY(20);
  InstallLocation.AutoSize := False;
  InstallLocation.ShowAccelChar := False;
  InstallLocation.WordWrap := True;
  InstallLocation.Width := WizardForm.ReadyMemo.Width;
  InstallLocation.Anchors := [akLeft, akTop, akRight];
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpReady then begin
    WizardForm.ReadyMemo.Visible := False;
    InstallLocation.Caption := WizardDirValue;
    InstallLocation.AdjustHeight;
  end;
end;

function HasMatchingMachine(FileName: String): Boolean;
var
  File: TFileStream;
  Buffer: AnsiString;
  HeaderOffset: Int64;
begin
  Result := not FileExists(FileName);
  if Result then
    exit;
  try
    File := TFileStream.Create(FileName, fmOpenRead or fmShareDenyNone);
    try
      SetLength(Buffer, 64);
      if File.Read(Buffer, 64) <> 64 then
        exit;
      if Copy(Buffer, 1, 2) <> 'MZ' then
        exit;
      HeaderOffset := Ord(Buffer[61]) + Ord(Buffer[62]) * 256 +
        Ord(Buffer[63]) * 65536 + Int64(Ord(Buffer[64])) * 16777216;
      if (HeaderOffset < 64) or (HeaderOffset > File.Size - 26) then
        exit;
      File.Position := HeaderOffset;
      SetLength(Buffer, 26);
      if File.Read(Buffer, 26) <> 26 then
        exit;
      Result := (Copy(Buffer, 1, 4) = 'PE'#0#0) and
        ((Ord(Buffer[23]) and 2) <> 0) and ((Ord(Buffer[24]) and 32) = 0) and
        (Ord(Buffer[25]) = 11) and (Ord(Buffer[26]) = 2) and
        (Ord(Buffer[5]) + Ord(Buffer[6]) * 256 = {#MatouPayloadMachine});
    finally
      File.Free;
    end;
  except
    Log('Unable to read the architecture of an existing Matou executable');
  end;
end;

function HasMatchingPayload(Directory: String): Boolean;
begin
  Directory := AddBackslash(Directory);
  Result := HasMatchingMachine(Directory + '码头.exe') and
    HasMatchingMachine(Directory + 'matou-native-runtime.exe');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  RegisteredDir: String;
begin
  Result := '';
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\com.matou.desktop_is1',
      'Inno Setup: App Path', RegisteredDir) and (RegisteredDir <> '') then
    if not HasMatchingPayload(RegisteredDir) then begin
      Result := CustomMessage('MatouOtherArchitecture');
      exit;
    end;
  if not HasMatchingPayload(WizardDirValue) then
    Result := CustomMessage('MatouOtherArchitecture');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ProtocolKey, ExistingCommand, OwnedCommand: String;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  ProtocolKey := 'Software\Classes\matou-notification';
  OwnedCommand := '"' + ExpandConstant('{app}\码头.exe') + '" "%1"';
  if RegQueryStringValue(HKCU, ProtocolKey + '\shell\open\command', '', ExistingCommand) and
     (CompareText(ExistingCommand, OwnedCommand) = 0) then
    RegDeleteKeyIncludingSubkeys(HKCU, ProtocolKey);
end;
