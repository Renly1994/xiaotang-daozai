# 本机 MSVC 构建环境（优先用 BuildTools，Community 缺桌面 CRT）。
#
# 用法：在仓库根目录执行（脚本含中文，必须以 UTF-8 with BOM 保存，否则 Windows PowerShell 5.1 会解析失败）
#   powershell -NoProfile -ExecutionPolicy Bypass -Command ". .\scripts\dev-msvc-env.ps1; cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc"
#
# 本机情况（2026-11-20 实测）：
# - C:\Program Files\Microsoft Visual Studio\18\Community 只装了 onecore 变体，
#   缺 VC\Tools\MSVC\<ver>\include 与 lib\x64（链接报 LNK1104 msvcrt.lib、C1083 vcruntime.h）。
# - 2026-11-20 重新装齐：C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools
#   （VCTools + Windows SDK 10.0.26100）。文档里旧的 18\BuildTools 在这台机器上已不存在，
#   所以这里不再写死路径，按候选列表 + vswhere 自动挑带 vcvars64.bat 的实例。
# 脚本建立标准桌面环境后，额外把 TEMP/TMP 指到仓库 target/link-tmp
# （避免链接阶段使用不可写或超长临时目录）。

$ErrorActionPreference = 'Stop'

$candidates = @(
    'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat',
    'C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat',
    'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat',
    'C:\Program Files\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
)
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vswhere) {
    foreach ($instance in (& $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)) {
        if ($instance) { $candidates += (Join-Path $instance.Trim() 'VC\Auxiliary\Build\vcvars64.bat') }
    }
}
$vcvars = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $vcvars) { throw "找不到 vcvars64.bat。候选：`n$($candidates -join "`n")" }
Write-Host "vcvars: $vcvars"

# vcvars 只在 cmd 进程内设置环境；用 set 把结果导回当前 PowerShell。
$capture = & cmd /c "`"$vcvars`" >nul 2>&1 && set" | Where-Object { $_ -match '^[A-Za-z_][A-Za-z0-9_]*=' }
foreach ($line in $capture) {
    $name, $value = $line -split '=', 2
    if ($name -and $value) { Set-Item -Path "Env:$name" -Value $value }
}

$linkTmp = Join-Path (Get-Location) 'target\link-tmp'
New-Item -ItemType Directory -Force -Path $linkTmp | Out-Null
$env:TEMP = $linkTmp
$env:TMP = $linkTmp

if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) { throw 'vcvars64.bat 已执行，但 PATH 里仍没有 cl.exe' }
if (-not $env:LIB -or -not ($env:LIB -like '*MSVC*lib\x64*')) { throw "LIB 未正确设置：$env:LIB" }

# cc-rs 会按 vswhere 自己挑 VS 实例，本机 Community 缺 CRT/SDK 头，必须把编译器钉死到
# BuildTools 的 cl.exe，否则 zstd-sys / ring 等 C 依赖会报「无法打开包括文件 limits.h」。
$env:CC = (Get-Command cl.exe).Source
$env:CXX = $env:CC
$env:AR = (Get-Command lib.exe).Source

Write-Host ("MSVC: {0}" -f (Get-Command cl.exe).Source)
Write-Host '环境已就绪：cl.exe / link.exe / rc.exe 与 INCLUDE/LIB 已设置（CC/CXX/AR 已钉死），TEMP 指向 target/link-tmp。'
