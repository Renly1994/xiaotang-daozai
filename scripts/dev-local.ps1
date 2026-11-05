<#
.SYNOPSIS
使用系统 Rust 或本项目 target/dev-tools 内的 Rust 构建、测试和运行岛仔。
.EXAMPLE
pwsh -File ./scripts/dev-local.ps1 Build
.EXAMPLE
pwsh -File ./scripts/dev-local.ps1 Run
#>
[CmdletBinding()]
param(
    [ValidateSet('Check', 'Build', 'Test', 'Run')]
    [string]$Action = 'Check',
    [switch]$LowMemory
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$localCargo = Join-Path $projectRoot 'target/dev-tools/cargo'
$localRustup = Join-Path $projectRoot 'target/dev-tools/rustup'
if (Test-Path -LiteralPath (Join-Path $localCargo 'bin/cargo.exe')) {
    $env:CARGO_HOME = $localCargo
    $env:RUSTUP_HOME = $localRustup
    $env:PATH = "$(Join-Path $localCargo 'bin');$env:PATH"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw '未找到 Rust。请安装 Rust stable 的 x86_64-pc-windows-msvc 工具链。'
}
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }
$taskCargoProfile=@()
if ($LowMemory) {
    $env:CARGO_BUILD_JOBS='1'
    $env:CARGO_INCREMENTAL='0'
    $taskProfile=if ($Action -eq 'Test') {'test'} else {'dev'}
    $taskCargoProfile=@('--config',"profile.$taskProfile.package.daozai.debug=0",'--config',"profile.$taskProfile.package.daozai.codegen-units=256")
}

# NuGet 版 Windows SDK 可放在 E 盘，不要求系统盘安装完整 SDK。
$sdkHeaders = Join-Path $projectRoot 'target/dev-tools/windows-sdk-cpp/c'
$sdkLibraries = Join-Path $projectRoot 'target/dev-tools/windows-sdk-x64/c'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$compiler = if (Test-Path -LiteralPath $vswhere) {
    & $vswhere -all -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
if ($compiler -and (Test-Path -LiteralPath (Join-Path $sdkLibraries 'um/x64/kernel32.lib'))) {
    $msvc = Get-ChildItem -LiteralPath (Join-Path $compiler 'VC/Tools/MSVC') -Directory |
        Sort-Object Name -Descending | Select-Object -First 1
    $includeVersion = Get-ChildItem -LiteralPath (Join-Path $sdkHeaders 'Include') -Directory |
        Sort-Object Name -Descending | Select-Object -First 1
    if (-not $msvc -or -not $includeVersion) { throw '本地 SDK 或 MSVC 文件不完整。' }
    $env:PATH = "$(Join-Path $msvc.FullName 'bin/Hostx64/x64');$(Join-Path $sdkHeaders "bin/$($includeVersion.Name)/x64");$env:PATH"
    $env:INCLUDE = (@((Join-Path $msvc.FullName 'include')) + @('ucrt','shared','um','winrt') |
        ForEach-Object { if ([IO.Path]::IsPathRooted($_)) { $_ } else { Join-Path $includeVersion.FullName $_ } }) -join ';'
    $env:LIB = @((Join-Path $msvc.FullName 'lib/x64'), (Join-Path $sdkLibraries 'um/x64'), (Join-Path $sdkLibraries 'ucrt/x64')) -join ';'
    $env:VCINSTALLDIR = Join-Path $compiler 'VC/'
    $env:VCToolsInstallDir = $msvc.FullName + '\'
    $env:VSCMD_ARG_TGT_ARCH = 'x64'
    $env:VSCMD_ARG_HOST_ARCH = 'x64'
    $env:WindowsSdkDir = $sdkHeaders + '\'
    $env:WindowsSDKVersion = $includeVersion.Name + '\'
    $env:RC = Join-Path $sdkHeaders "bin/$($includeVersion.Name)/x64/rc.exe"
    $taskTemp = Join-Path $projectRoot 'target/dev-temp'
    New-Item -ItemType Directory -Path $taskTemp -Force | Out-Null
    $env:TEMP = $taskTemp
    $env:TMP = $taskTemp
}

Push-Location -LiteralPath $projectRoot
try {
    if ($Action -eq 'Check') {
        & rustc --version
        if ($LASTEXITCODE -ne 0) { throw 'Rust 工具链不可用。' }
        & cargo --version
        if ($LASTEXITCODE -ne 0) { throw 'Cargo 不可用。' }
        if (-not (Test-Path -LiteralPath $vswhere)) { throw '未找到 Visual Studio Build Tools。' }
        if (-not $compiler) { throw '请在 Visual Studio Installer 中安装 x64/x86 C++ 编译工具。' }
        Write-Output "MSVC: $compiler"
        if (Test-Path -LiteralPath (Join-Path $sdkLibraries 'um/x64/kernel32.lib')) {
            Write-Output "Windows SDK (NuGet): $sdkHeaders"
            return
        }
        $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/Lib'
        $sdk = Get-ChildItem -LiteralPath $sdkRoot -Directory -ErrorAction SilentlyContinue |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'um/x64/kernel32.lib') }
        if (-not $sdk) { throw '未找到可用于 x64 编译的 Windows SDK。' }
        $sdk | ForEach-Object { Write-Output "Windows SDK: $($_.Name)" }
        return
    }

    # 编译、应用数据均留在 Git 已忽略的 target 下；不读取日常岛仔的设置。
    $env:DAOZAI_DATA_DIR = Join-Path $projectRoot 'target/dev-data'
    if ($Action -eq 'Run' -and (Get-Process daozai -ErrorAction SilentlyContinue)) {
        throw '岛仔已在运行。请通过托盘菜单退出后重试；原版的单实例锁不支持同时运行两个实例。'
    }
    switch ($Action) {
        { $_ -in 'Build','Run' } {
            & cargo build --locked --offline @taskCargoProfile --bin daozai
            if ($LASTEXITCODE -ne 0) { throw '构建岛仔失败。' }
            if ($Action -eq 'Run') {
                # Windows 锁定运行中的 EXE；单独复制一份，避免阻塞随后执行的测试。
                $runDirectory = Join-Path $projectRoot 'target/dev-run'
                New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
                $runExecutable = Join-Path $runDirectory 'daozai.exe'
                Copy-Item -LiteralPath (Join-Path $projectRoot 'target/debug/daozai.exe') -Destination $runExecutable -Force
                & $runExecutable
            }
        }
        'Test' { & cargo test --locked --offline @taskCargoProfile --all-targets --features record -- --test-threads=1 }
    }
    if ($LASTEXITCODE -ne 0) { throw "Cargo $Action 失败（退出码 $LASTEXITCODE），请查看上方错误。若提示缺少离线依赖，请先执行 cargo fetch --locked。" }
} finally {
    Pop-Location
}
