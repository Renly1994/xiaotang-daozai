# 本机 MSVC 构建环境（VS 18 BuildTools 完整、Community 缺桌面 CRT）。
#
# 用法：在仓库根目录执行
#   . .\scripts\dev-msvc-env.ps1
#   cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc
#
# 本机情况（2026-11-20 实测）：
# - C:\Program Files\Microsoft Visual Studio\18\Community 只装了 onecore 变体，
#   缺 VC\Tools\MSVC\<ver>\include 与 lib\x64（链接报 LNK1104 msvcrt.lib、C1083 vcruntime.h）。
# - C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools 完整，vcvars64.bat 可用。
# 因此这里直接调用 BuildTools 的 vcvars64.bat 建立标准桌面环境，并额外把
# TEMP/TMP 指到仓库 target/link-tmp（避免链接阶段使用不可写或超长临时目录）。

$ErrorActionPreference = 'Stop'

$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) { throw "找不到 vcvars64.bat：$vcvars" }

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
