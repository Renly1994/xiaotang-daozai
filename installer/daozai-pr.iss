; 岛仔 PR 版（液态玻璃）安装包 —— Inno Setup 7
; 编译：ISCC.exe daozai-pr.iss
; 用于 VMMap 内存对比：简化正式包，去掉浏览器桥/开机自启/注册表等会引入额外进程的部件，
; 只装主 exe + ico，用户数据在 %APPDATA%\Daozai（正式路径）。
; 与上游基准包用同一个 AppId/安装目录 —— 两者会互相覆盖，对比时一次只装一个。

#define MyAppName "岛仔 Daozai（PR 液态玻璃）"
#define MyAppVersion "0.6.66"
#define MyAppBuild "pr-liquid-glass-vendor"

[Setup]
AppId={{A58C1D9E-4F02-4B6A-9C33-1E7B5D2F8A44}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}（{#MyAppBuild}）
AppPublisher=自定义构建（PR）
DefaultDirName={autopf}\Daozai
DirExistsWarning=no
AppendDefaultDirName=no
PrivilegesRequired=admin
DefaultGroupName=岛仔 PR
DisableProgramGroupPage=yes
OutputDir=.
OutputBaseFilename=daozai-setup-{#MyAppBuild}
SetupIconFile=..\assets\daozai.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UninstallDisplayName={#MyAppName}（{#MyAppBuild}）
UninstallDisplayIcon={app}\daozai.ico

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "其他："; Flags: unchecked

[Files]
Source: "..\target\release\daozai.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\daozai.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\岛仔 PR 液态玻璃"; Filename: "{app}\daozai.exe"; IconFilename: "{app}\daozai.ico"
Name: "{autodesktop}\岛仔 PR 液态玻璃"; Filename: "{app}\daozai.exe"; IconFilename: "{app}\daozai.ico"; Tasks: desktopicon

[UninstallDelete]
Type: files; Name: "{app}\PR_BUILD.txt"

[Run]
Filename: "{app}\daozai.exe"; Description: "启动岛仔"; Flags: nowait postinstall skipifsilent

[Code]
// 只在首次安装时铺默认设置：液态玻璃 + 兼容渲染(D3D12) + 独显优先，开箱即测完整玻璃内存。
// 已存在 settings.txt 就绝不覆盖（VMMap 对比时反复装卸，保留测试者调过的参数）。
procedure CurStepChanged(CurStep: TSetupStep);
var
  dir, file: string;
begin
  if CurStep = ssPostInstall then
  begin
    dir := GetEnv('APPDATA') + '\Daozai';
    ForceDirectories(dir);
    file := dir + '\settings.txt';
    if not FileExists(file) then
      SaveStringToFile(file,
        'skin=liquid_glass' + #13#10 +
        'render_compat=1' + #13#10 +
        'liquid_prefer_dgpu=1' + #13#10 +
        'island_shape=pill' + #13#10 +
        'update_check=0' + #13#10,
        False);
    SaveStringToFile(ExpandConstant('{app}\PR_BUILD.txt'), '{#MyAppBuild}', False);
  end;
end;
