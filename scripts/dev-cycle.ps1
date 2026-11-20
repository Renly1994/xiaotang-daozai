<#
.SYNOPSIS
  岛仔（Rust + egui）开发循环一键入口：构建 → 测试 → E2E → 交付 → 提交（不 push）。

.DESCRIPTION
  把「改代码 → 构建 → 测试 → 交付 → 跑 E2E → 提交」固化成可分段执行的脚本：
  每个步骤都先做前置校验并给出人话错误；结果写入 target\dev-cycle\<时间戳>.json，
  同目录还有 <时间戳>.log 与 <时间戳>\ 下各步骤的原始日志。

  文件编码（重要，已踩两次）：
  本文件必须是 UTF-8 with BOM，否则 Windows PowerShell 5.1 会按本地代码页解析中文而报错
  （典型症状：字符串缺少终止符 / 无法识别的标记）。脚本每次启动都会自检：读自身字节前三位，
  不是 EF BB BF 就以退出码 19 停止（实现见函数 Test-ScriptUtf8Bom）。等价的命令行自检：
    $b=[System.IO.File]::ReadAllBytes('scripts\dev-cycle.ps1')
    ($b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF)   # 必须 True
  修复（重写为带 BOM，注意 UTF8Encoding($true)）：
    $p='scripts\dev-cycle.ps1'; $t=[System.IO.File]::ReadAllText($p,[System.Text.Encoding]::UTF8)
    [System.IO.File]::WriteAllText($p,$t,(New-Object System.Text.UTF8Encoding($true)))

  退出码：
     0 成功 / DryRun 完成
    10 环境缺失（不在仓库根、cargo/node/git 缺失、关键脚本缺失）
    11 锁屏（检测到 LogonUI），E2E 会截到锁屏背景并误判
    12 存在「不是当前构建产物」启动的岛仔实例（脚本不会替你停别人的程序）
    13 构建产物 exe 被占用且停不掉（杀毒/其它句柄）
    14 cargo build 失败
    15 测试失败（cargo test --lib 或 node --test）
    16 交付校验失败（启动失败 / 进程路径不符 / SHA256 不符）
    17 E2E 失败（agent-e2e*.ps1 的 2/3/4 已在输出里映射成人话）
    18 git 提交失败
    19 本脚本缺少 UTF-8 BOM
    20 参数组合不合法
    21 找不到构建产物（先跑 -Step Build）
     1 未预期异常（看记录里 steps[].detail）

.EXAMPLE
  # 只自检（BOM / 解析 / 工具 / 参数 / 记录写入；不构建、不启停岛仔）
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-cycle.ps1 -Step SelfCheck

.EXAMPLE
  # 完整链路（含提交；不 push）
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-cycle.ps1 -Step All -Message "修复提问卡片进度"

.EXAMPLE
  # 只跑快速测试（ui_tests + agent + node）
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-cycle.ps1 -Step Verify -Fast

.EXAMPLE
  # 先看计划：只做前置校验 + 打印将执行的命令，不构建/不启停/不提交
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-cycle.ps1 -Step All -DryRun

.NOTES
  使用说明：docs/superpowers/research/2026-11-20-dev-cycle-usage.md
  本文件必须以 UTF-8 with BOM 保存（见上面 DESCRIPTION 的自检与修复命令）。
#>
[CmdletBinding()]
param(
    # 要执行的步骤，可多选（逗号或空格分隔，如 -Step Build,Verify / -Step Build Verify）；
    # All 展开为 Build,Verify,E2E,Deliver（给 -Message 或 -IncludeCommit 时追加 Commit）
    [string[]]$Step = @(),

    # -Step All 的简写（与 -Message 同用时自动追加 Commit）
    [switch]$All,

    # Verify 只跑 ui_tests + agent 两个过滤器的 cargo test 与 node 测试
    [switch]$Fast,

    # E2E 场景：agent-e2e.ps1 的 all/approval/question/question-multi/question-multi-turn/done，
    # 或 agent-e2e-repro.ps1 的 done-repro/jump-repro/scroll-repro/banner-jump-repro
    [ValidateSet('all', 'approval', 'question', 'question-multi', 'question-multi-turn', 'done',
        'done-repro', 'jump-repro', 'scroll-repro', 'banner-jump-repro')]
    [string]$Scenario = 'all',

    # 提交文案（subject）。与 -Step All 同用时自动追加 Commit 步骤；不传则 All 不提交。
    [string]$Message,

    # Deliver 只核对 exe 哈希/快捷方式，不启动岛仔
    [switch]$NoStart,

    # 只看计划：做全部只读前置校验并打印命令，不构建、不停/启岛仔、不跑测试、不提交
    [switch]$DryRun,

    # -Step All 时强制包含 Commit（即使没给 -Message，会生成默认文案）
    [switch]$IncludeCommit,

    # 交付启动后等待进程出现的秒数
    [ValidateRange(5, 600)]
    [int]$StartWaitSeconds = 20,

    # E2E 场景内部超时（传给 agent-e2e*.ps1 的 -WaitSeconds）
    [ValidateRange(10, 1800)]
    [int]$E2EWaitSeconds = 90
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# 统一控制台输出编码，便于 5.1/7 下捕获子进程的中文输出（E2E 的「截图与结果：」行）。
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }
try { $OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }

# ---------------------------------------------------------------------------
# 基础工具函数
# ---------------------------------------------------------------------------

$script:Log = @()

function Write-Log {
    param([string]$Message, [string]$Color)
    $line = '[{0}] {1}' -f (Get-Date -Format 'HH:mm:ss'), $Message
    $script:Log += $line
    if ($Color) { Write-Host $line -ForegroundColor $Color } else { Write-Host $line }
}

function Format-Seconds {
    param([double]$Seconds)
    return ('{0:N1}s' -f $Seconds)
}

function Get-Sha256 {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    try { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
    catch { return $null }
}

function Test-ScriptUtf8Bom {
    $path = $script:ScriptPath
    if (-not $path -or -not (Test-Path -LiteralPath $path)) {
        return [pscustomobject]@{ ok = $false; path = $path; bytes = $null; reason = '找不到脚本自身路径' }
    }
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $ok = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
    return [pscustomobject]@{ ok = $ok; path = $path; bytes = $bytes; reason = '' }
}

function Get-CycleRepoRoot {
    $roots = @()
    if ($PSScriptRoot) { $roots += (Split-Path -Parent $PSScriptRoot) }
    $roots += (Get-Location).Path
    foreach ($r in $roots) {
        if ($r -and (Test-Path -LiteralPath (Join-Path $r 'Cargo.toml'))) {
            return [System.IO.Path]::GetFullPath($r)
        }
    }
    Fail 10 ("找不到岛仔仓库根（应包含 Cargo.toml）。脚本在 $PSScriptRoot，当前目录 $(Get-Location)。`n" +
        '请把本脚本放在 <仓库>\scripts\ 下，或在仓库根目录执行。')
}

function Get-DaozaiProcesses {
    $list = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -like 'daozai*' })
    $out = @()
    foreach ($p in $list) {
        $path = $null
        try { $path = $p.Path } catch { $path = $null }
        $managed = $false
        if ($path) { $managed = ($path -ieq $script:ExePath) }
        $out += [pscustomobject]@{ Id = $p.Id; Name = $p.ProcessName; Path = $path; Managed = $managed }
    }
    return $out
}

function Test-ExeLocked {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    try {
        $fs = [System.IO.File]::Open($Path, 'Open', 'ReadWrite', 'None')
        $fs.Close()
        return $false
    } catch {
        return $true
    }
}

function Assert-Tool {
    param([string]$Name, [string]$Hint)
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $cmd) {
        Fail 10 ("找不到 $Name 命令。`n" + $Hint)
    }
    return $cmd.Source
}

function Assert-File {
    param([string]$Path, [string]$Label, [int]$Code = 10)
    if (-not (Test-Path -LiteralPath $Path)) {
        Fail $Code ("找不到 $Label：$Path")
    }
}

function Assert-DesktopUnlocked {
    $logon = @(Get-Process -Name 'LogonUI' -ErrorAction SilentlyContinue)
    if ($logon.Count -gt 0) {
        $pids = ($logon | ForEach-Object { $_.Id }) -join ', '
        Fail 11 ("当前工作站处于锁屏状态（检测到 LogonUI，PID $pids）。`n" +
            'E2E 依赖真实桌面截图与鼠标点击，锁屏时只能截到锁屏背景，会得到一堆假失败。请先解锁桌面再跑。')
    }
    Write-Log '  桌面未锁屏（未检测到 LogonUI）'
}

function Get-PowerShellExe {
    $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (Test-Path -LiteralPath $ps) { return $ps }
    $cmd = Get-Command powershell.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $cmd = Get-Command pwsh -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    Fail 10 '找不到 powershell.exe / pwsh，无法调用 E2E 脚本。'
}

function Get-DaozaiShortcut {
    $desktops = @(
        (Join-Path $env:USERPROFILE 'Desktop'),
        (Join-Path $env:PUBLIC 'Desktop')
    )
    $shell = $null
    try { $shell = New-Object -ComObject WScript.Shell } catch { $shell = $null }
    foreach ($d in $desktops) {
        if (-not (Test-Path -LiteralPath $d)) { continue }
        $links = @(Get-ChildItem -LiteralPath $d -Filter '*.lnk' -ErrorAction SilentlyContinue)
        foreach ($lnk in $links) {
            $target = $null
            if ($shell) {
                try { $target = $shell.CreateShortcut($lnk.FullName).TargetPath } catch { $target = $null }
            }
            if ($target -and ($target -like '*daozai*.exe')) {
                return [pscustomobject]@{
                    path    = $lnk.FullName
                    target  = $target
                    matches = ($target -ieq $script:ExePath)
                }
            }
            if (-not $target -and $lnk.Name -like '*岛仔*') {
                return [pscustomobject]@{ path = $lnk.FullName; target = $null; matches = $null }
            }
        }
    }
    return $null
}

function Stop-ManagedDaozai {
    # ForeignPolicy：遇到「不是本仓库 exe 启动的岛仔」怎么处理。
    #   fail（默认，Deliver/E2E 用）：单实例互斥体会互踢，必须让用户手动收掉；
    #   warn（Build 用）：构建只关心本仓库 exe 是否被占用，别家副本不挡构建，警告即可。
    param(
        [string]$Reason,
        [ValidateSet('warn', 'fail')]
        [string]$ForeignPolicy = 'fail'
    )
    $procs = @(Get-DaozaiProcesses)
    if ($procs.Count -eq 0) {
        Write-Log '  没有岛仔实例在运行'
        return
    }
    $foreign = @($procs | Where-Object { -not $_.Managed })
    if ($foreign.Count -gt 0) {
        $desc = ($foreign | ForEach-Object {
                $fp = if ($_.Path) { $_.Path } else { '<拿不到路径，可能权限不足>' }
                "PID $($_.Id)（$($_.Name)）路径 $fp"
            }) -join '; '
        if ($ForeignPolicy -eq 'fail') {
            Fail 12 ("检测到不是当前构建产物启动的岛仔实例：$desc`n" +
                "本脚本只停 $($script:ExePath) 启动的实例，不会替你停别人的程序。`n" +
                '请手动关掉这些实例后重跑；只想看计划可加 -DryRun。')
        }
        Write-Log ("  警告：有非本仓库 exe 启动的岛仔实例（{0}）；构建只关心产物 exe 是否被占用，本次不停它。" -f $desc) 'Yellow'
    }
    $managed = @($procs | Where-Object { $_.Managed })
    if ($managed.Count -eq 0) {
        Write-Log '  没有本仓库 exe 启动的岛仔实例需要停止'
        return
    }
    $pids = ($managed | ForEach-Object { $_.Id }) -join ', '
    if ($DryRun) {
        Write-Log ("  [DryRun] 真实运行时会在「$Reason」先停止岛仔实例：PID $pids")
        return
    }
    Write-Log ("  「$Reason」停止岛仔实例：PID $pids")
    foreach ($p in $managed) {
        try { Stop-Process -Id $p.Id -Force -ErrorAction Stop } catch {
            Write-Log ("  停止 PID {0} 失败：{1}" -f $p.Id, $_.Exception.Message) 'Yellow'
        }
    }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 10) {
        $still = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
                $_.Id -in $managed.Id
            })
        if ($still.Count -eq 0) { break }
        Start-Sleep -Milliseconds 200
    }
    $remain = @($managed | Where-Object { Get-Process -Id $_.Id -ErrorAction SilentlyContinue })
    if ($remain.Count -gt 0) {
        $desc = ($remain | ForEach-Object { "PID $($_.Id)" }) -join ', '
        Fail 12 ("停止岛仔后仍有实例存活：$desc。请手动结束（任务管理器 / Stop-Process -Id <pid> -Force）后重跑。")
    }
    Write-Log '  本仓库的岛仔已退出'
}

# ---------------------------------------------------------------------------
# 记录与失败
# ---------------------------------------------------------------------------

function Save-CycleRecord {
    param([int]$Code = 0, [string]$ErrorMessage = '')
    if ($script:RecordSaved) { return }
    $script:RecordSaved = $true
    if (-not $script:State) { return }
    $script:State.exitCode = $Code
    if ($ErrorMessage) { $script:State.error = $ErrorMessage }
    $script:State.finishedAt = (Get-Date).ToString('s')
    $script:State.totalSeconds = [Math]::Round(((Get-Date) - $script:StartedAt).TotalSeconds, 1)
    $script:State.log = $script:Log
    try {
        New-Item -ItemType Directory -Force -Path $script:RecordDir | Out-Null
        $json = $script:State | ConvertTo-Json -Depth 12
        [System.IO.File]::WriteAllText($script:RecordPath, $json, (New-Object System.Text.UTF8Encoding($false)))
        [System.IO.File]::WriteAllText($script:LogPath, (($script:Log -join [Environment]::NewLine) + [Environment]::NewLine), (New-Object System.Text.UTF8Encoding($false)))
        Write-Host ''
        Write-Host ("记录：{0}" -f $script:RecordPath) -ForegroundColor DarkCyan
        Write-Host ("日志：{0}" -f $script:LogPath) -ForegroundColor DarkCyan
    } catch {
        Write-Host ("写运行记录失败：{0}" -f $_.Exception.Message) -ForegroundColor Yellow
    }
}

function Fail {
    param([int]$Code, [string]$ErrorMessage)
    if ($script:CurrentStep) {
        $script:CurrentStep.status = 'failed'
        $script:CurrentStep.exitCode = $Code
        $script:CurrentStep.detail = $ErrorMessage
    }
    Write-Host ''
    Write-Host ("失败（退出码 {0}）：{1}" -f $Code, $ErrorMessage) -ForegroundColor Red
    Write-Log ("FAIL[{0}] {1}" -f $Code, $ErrorMessage) 'Red'
    Save-CycleRecord $Code $ErrorMessage
    exit $Code
}

function Invoke-CycleStep {
    param([string]$Name, [scriptblock]$Body)
    $entry = [ordered]@{
        name      = $Name
        status    = 'running'
        startedAt = (Get-Date).ToString('s')
        seconds   = 0.0
        detail    = ''
        exitCode  = 0
    }
    $script:State.steps += $entry
    $script:CurrentStep = $entry
    Write-Host ''
    Write-Host ("========== [{0}] ==========" -f $Name) -ForegroundColor Cyan
    Write-Log ("步骤开始：{0}" -f $Name) 'Cyan'
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        & $Body
    } catch {
        $entry.status = 'failed'
        $entry.exitCode = 1
        $entry.detail = $_.Exception.Message
        $sw.Stop()
        $entry.seconds = [Math]::Round($sw.Elapsed.TotalSeconds, 1)
        Fail 1 ("步骤 $Name 出现未预期异常：$($_.Exception.Message)")
    }
    $sw.Stop()
    $entry.seconds = [Math]::Round($sw.Elapsed.TotalSeconds, 1)
    if ($entry.status -eq 'running') {
        if ($DryRun) { $entry.status = 'dry-run' } else { $entry.status = 'ok' }
    }
    $script:CurrentStep = $null
    Write-Log ("步骤完成：{0}（{1}，{2}）" -f $Name, $entry.status, (Format-Seconds $entry.seconds)) 'Green'
}

function Invoke-NativeLogged {
    param(
        [string]$Label,
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$LogPath
    )
    $shown = ('{0} {1}' -f $FilePath, ($Arguments -join ' ')).Trim()
    Write-Log ("  执行：{0}" -f $shown) 'DarkGray'
    # 日志先建好并写头：Tee 在失败/中断的流水线里可能根本没落盘（2026-11-20 真实踩坑：
    # 失败记录里指向 build.log，但文件不存在）。这里用 StreamWriter 自己写，UTF-8 无 BOM，5.1/7 一致。
    $logDir = Split-Path -Parent $LogPath
    if ($logDir -and -not (Test-Path -LiteralPath $logDir)) { New-Item -ItemType Directory -Force -Path $logDir | Out-Null }
    $writer = $null
    $lines = New-Object System.Collections.ArrayList
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $prevEap = $ErrorActionPreference
    $code = 1
    try {
        $writer = New-Object System.IO.StreamWriter($LogPath, $false, (New-Object System.Text.UTF8Encoding($false)))
        $writer.WriteLine(('# dev-cycle {0} | {1} | 命令：{2}' -f (Get-Date).ToString('s'), $Label, $shown))
        $writer.Flush()

        # 关键修复（2026-11-20）：cargo/rustc 把 warning 写到 stderr。dev-msvc-env.ps1 把
        # $ErrorActionPreference 设成 Stop，`2>&1 | ...` 里的 stderr 会被包成 NativeCommandError
        # 终止流水线 → 假失败（0.8s「构建失败」、退出码 14）。这里临时改 Continue，
        # 只以 $LASTEXITCODE 判定成功/失败；stderr 内容照常收进日志与屏幕输出。
        $ErrorActionPreference = 'Continue'
        $global:LASTEXITCODE = 0
        & $FilePath @Arguments 2>&1 | ForEach-Object {
            $line = "$_"
            # Windows PowerShell 5.1 把原生命令 stderr 的一条空行包成 ErrorRecord 后，字符串化会变成
            # "System.Management.Automation.RemoteException"，会污染日志与屏幕。空行只要写成空行。
            if ($_ -is [System.Management.Automation.ErrorRecord]) {
                $msg = $_.Exception.Message
                if ($null -ne $msg -and $msg.Trim().Length -gt 0) { $line = $msg } else { $line = '' }
            }
            $writer.WriteLine($line)
            [void]$lines.Add($line)
            Write-Host ("    " + $line)
        }
        $code = $LASTEXITCODE
    } catch {
        $text = ($_ | Out-String)
        foreach ($l in ($text -split "`r?`n")) {
            if ($l -ne '') {
                [void]$lines.Add($l)
                if ($writer) { try { $writer.WriteLine($l) } catch { } }
            }
        }
        Write-Host ("    " + $text)
        $code = 1
    } finally {
        $ErrorActionPreference = $prevEap
        if ($writer) { try { $writer.Dispose() } catch { } }
        $sw.Stop()
    }
    $tail = @($lines | Select-Object -Last 40 | ForEach-Object { "$_" })
    return [pscustomobject]@{
        code    = $code
        seconds = [Math]::Round($sw.Elapsed.TotalSeconds, 1)
        lines   = @($lines)
        tail    = $tail
        command = $shown
        log     = $LogPath
    }
}

# ---------------------------------------------------------------------------
# 测试结果解析
# ---------------------------------------------------------------------------

function Get-CargoTestSummary {
    param([string[]]$Lines)
    $passed = 0; $failed = 0; $suites = 0; $ok = $true; $found = $false
    foreach ($raw in $Lines) {
        $s = "$raw"
        $m = [regex]::Match($s, 'test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;')
        if ($m.Success) {
            $found = $true
            $passed += [int]$m.Groups[2].Value
            $failed += [int]$m.Groups[3].Value
            $suites++
            if ($m.Groups[1].Value -ne 'ok') { $ok = $false }
        }
    }
    return [pscustomobject]@{ found = $found; ok = $ok; passed = $passed; failed = $failed; suites = $suites }
}

function Get-NodeTestSummary {
    param([string[]]$Lines)
    $pass = $null; $fail = $null; $tests = $null
    foreach ($raw in $Lines) {
        $s = "$raw"
        $m = [regex]::Match($s, '(?:#|ℹ)\s*pass\s+(\d+)')
        if ($m.Success) { $pass = [int]$m.Groups[1].Value }
        $m = [regex]::Match($s, '(?:#|ℹ)\s*fail\s+(\d+)')
        if ($m.Success) { $fail = [int]$m.Groups[1].Value }
        $m = [regex]::Match($s, '(?:#|ℹ)\s*tests\s+(\d+)')
        if ($m.Success) { $tests = [int]$m.Groups[1].Value }
    }
    return [pscustomobject]@{ pass = $pass; fail = $fail; tests = $tests }
}

# ---------------------------------------------------------------------------
# 各步骤实现
# ---------------------------------------------------------------------------

function Invoke-SelfCheckStep {
    $failures = @()
    $warnings = @()

    # 1) 自身 BOM
    $bom = Test-ScriptUtf8Bom
    if ($bom.ok) {
        Write-Log '  [OK] 本脚本是 UTF-8 with BOM（EF BB BF）' 'Green'
    } else {
        $failures += ('本脚本缺少 UTF-8 BOM（当前前三位：{0}）。修复：' -f (($bom.bytes | Select-Object -First 3 | ForEach-Object { '{0:X2}' -f $_ }) -join ' '))
        Write-Log '  [FAIL] 本脚本缺少 UTF-8 BOM' 'Red'
    }

    # 2) 语法可解析
    $parseErrors = $null
    try {
        $tokens = $null; $parseErrors = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile($script:ScriptPath, [ref]$tokens, [ref]$parseErrors)
        if ($parseErrors -and $parseErrors.Count -gt 0) {
            $failures += ('语法解析错误 {0} 处，第一处：{1}' -f $parseErrors.Count, $parseErrors[0].Message)
            Write-Log ('  [FAIL] 语法解析错误：{0}' -f $parseErrors[0].Message) 'Red'
        } else {
            Write-Log '  [OK] 语法解析通过' 'Green'
        }
    } catch {
        $failures += ('语法解析调用失败：{0}' -f $_.Exception.Message)
    }

    # 3) scripts\*.ps1 的 BOM 体检（含中文却没有 BOM 的，5.1 会解析失败）
    $noBom = @()
    $scriptsDir = Join-Path $script:RepoRoot 'scripts'
    foreach ($f in @(Get-ChildItem -LiteralPath $scriptsDir -Filter '*.ps1' -ErrorAction SilentlyContinue)) {
        $bytes = [System.IO.File]::ReadAllBytes($f.FullName)
        $hasBom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
        if ($hasBom) { continue }
        $nonAscii = $false
        foreach ($b in $bytes) { if ($b -gt 127) { $nonAscii = $true; break } }
        if ($nonAscii) { $noBom += $f.Name }
    }
    if ($noBom.Count -gt 0) {
        $warnings += ('以下 scripts\*.ps1 含非 ASCII 但没有 BOM（PS 5.1 下可能乱码）：' + ($noBom -join ', '))
        Write-Log ('  [WARN] 缺 BOM 的脚本：{0}' -f ($noBom -join ', ')) 'Yellow'
    } else {
        Write-Log '  [OK] scripts\*.ps1 的 BOM 体检通过' 'Green'
    }

    # 4) 关键文件
    $required = @(
        @{ path = (Join-Path $script:RepoRoot 'Cargo.toml'); label = 'Cargo.toml' },
        @{ path = $script:MsvcEnvScript; label = 'scripts\dev-msvc-env.ps1' },
        @{ path = (Join-Path $script:RepoRoot 'scripts\agent-e2e.ps1'); label = 'scripts\agent-e2e.ps1' },
        @{ path = (Join-Path $script:RepoRoot 'scripts\agent-e2e-repro.ps1'); label = 'scripts\agent-e2e-repro.ps1' },
        @{ path = (Join-Path $script:RepoRoot 'tests\agent-notices.test.mjs'); label = 'tests\agent-notices.test.mjs' }
    )
    foreach ($r in $required) {
        if (Test-Path -LiteralPath $r.path) {
            Write-Log ("  [OK] 存在 {0}" -f $r.label) 'Green'
        } else {
            $failures += ("缺少文件：{0}（{1}）" -f $r.label, $r.path)
            Write-Log ("  [FAIL] 缺少 {0}" -f $r.label) 'Red'
        }
    }

    # 5) 工具链
    foreach ($tool in @('cargo', 'rustc', 'node', 'git')) {
        $cmd = Get-Command $tool -ErrorAction SilentlyContinue
        if ($cmd) {
            Write-Log ("  [OK] {0} → {1}" -f $tool, $cmd.Source) 'Green'
        } else {
            $failures += ("找不到 {0} 命令（PATH 里没有）" -f $tool)
            Write-Log ("  [FAIL] 找不到 {0}" -f $tool) 'Red'
        }
    }
    $ps5 = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (Test-Path -LiteralPath $ps5) { Write-Log ("  [OK] Windows PowerShell 5.1 → {0}" -f $ps5) 'Green' }
    else { $warnings += '找不到 Windows PowerShell 5.1（E2E 将退回 pwsh，可能不兼容）'; Write-Log '  [WARN] 找不到 powershell.exe 5.1' 'Yellow' }

    # 6) 辅助函数自测（不产生副作用）
    $tmp = Join-Path $env:TEMP ('dev-cycle-selfcheck-{0}.tmp' -f $PID)
    try {
        [System.IO.File]::WriteAllText($tmp, 'abc')
        $expected = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
        $sha = Get-Sha256 $tmp
        if ($sha -eq $expected) { Write-Log '  [OK] Get-Sha256 自测（abc）通过' 'Green' }
        else { $failures += ('Get-Sha256 自测失败：{0}' -f $sha); Write-Log '  [FAIL] Get-Sha256 自测失败' 'Red' }

        $fs = [System.IO.File]::Open($tmp, 'Open', 'ReadWrite', 'None')
        $locked = Test-ExeLocked $tmp
        $fs.Close()
        $unlocked = (-not (Test-ExeLocked $tmp))
        if ($locked -and $unlocked) { Write-Log '  [OK] Test-ExeLocked 自测（占用/未占用）通过' 'Green' }
        else { $failures += ('Test-ExeLocked 自测失败：locked={0} unlocked={1}' -f $locked, $unlocked); Write-Log '  [FAIL] Test-ExeLocked 自测失败' 'Red' }
    } finally {
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
    }

    # 6b) 原生命令 stderr 回归自测：cargo/rustc 的 warning 走 stderr；EA=Stop 时若被当
    # NativeCommandError 终止流水线，就会出现「0.8s 构建失败」的假失败（2026-11-20 真实踩坑）。
    # 这里用 cmd.exe 向 stderr 写一行警告，要求退出码 0、内容被收进日志，且日志文件确实落盘。
    $stderrLog = Join-Path $script:RecordRunDir 'selfcheck-stderr.log'
    $probe = Invoke-NativeLogged -Label 'stderr-probe' -FilePath 'cmd.exe' -Arguments @('/c', 'echo native-stderr-warning 1>&2') -LogPath $stderrLog
    $probeText = ($probe.lines -join ' ')
    if ($probe.code -eq 0 -and $probeText -match 'native-stderr-warning' -and (Test-Path -LiteralPath $stderrLog)) {
        Write-Log '  [OK] 原生命令 stderr 警告未判失败（EA=Stop 回归自测）' 'Green'
        Remove-Item -LiteralPath $stderrLog -Force -ErrorAction SilentlyContinue
    } else {
        $failures += ('原生命令 stderr 回归自测失败：code={0} lines={1}' -f $probe.code, $probeText)
        Write-Log '  [FAIL] 原生命令 stderr 回归自测失败' 'Red'
    }

    # 7) 运行环境画像（只报信息，不算失败）
    $procs = @(Get-DaozaiProcesses)
    $managed = @($procs | Where-Object { $_.Managed })
    Write-Log ("  岛仔实例：共 {0} 个，其中当前构建产物启动的 {1} 个" -f $procs.Count, $managed.Count)
    foreach ($p in $procs) {
        $tag = if ($p.Managed) { '受管' } else { '外来' }
        Write-Log ("    - [{0}] PID {1}（{2}）路径 {3}" -f $tag, $p.Id, $p.Name, $p.Path)
    }
    $shortcut = Get-DaozaiShortcut
    if ($shortcut) {
        Write-Log ("  快捷方式：{0} → {1}" -f $shortcut.path, $shortcut.target)
    }

    # 8) 参数/步骤解析（会做合法性校验，非法直接 20）
    $stages = Resolve-StageList
    Write-Log ("  解析后的步骤：{0}" -f ($stages -join ' → '))
    Write-Log ("  DryRun={0} Fast={1} Scenario={2} NoStart={3}" -f [bool]$DryRun, [bool]$Fast, $Scenario, [bool]$NoStart)
    $template = New-CommitMessage
    Write-Log ("  提交模板自测（subject）：{0}" -f $template.subject)

    Write-Host ''
    Write-Host '退出码表：0 成功 / 10 环境缺失 / 11 锁屏 / 12 外来实例 / 13 exe 被占用 / 14 构建失败 / 15 测试失败 / 16 交付校验失败 / 17 E2E 失败 / 18 提交失败 / 19 缺 BOM / 20 参数不合法 / 21 缺构建产物'
    if ($warnings.Count -gt 0) {
        Write-Host ''
        Write-Log ('警告 {0} 条：' -f $warnings.Count) 'Yellow'
        foreach ($w in $warnings) { Write-Log ("  - {0}" -f $w) 'Yellow' }
    }
    if ($failures.Count -gt 0) {
        $code = 10
        if (-not $bom.ok) { $code = 19 }
        Fail $code ('自检未通过：' + ($failures -join '；'))
    }
    Write-Log '自检全部通过' 'Green'
}

function Invoke-BuildStep {
    Assert-Tool 'cargo' '安装 Rust（rustup）或把 %USERPROFILE%\.cargo\bin 加进 PATH。' | Out-Null
    Assert-Tool 'rustc' 'rustc 不在 PATH；检查 rustup 安装。' | Out-Null
    Assert-File $script:MsvcEnvScript 'scripts\dev-msvc-env.ps1'

    $busy = @(Get-Process -Name 'cargo', 'rustc', 'rustdoc' -ErrorAction SilentlyContinue)
    if ($busy.Count -gt 0) {
        $desc = ($busy | ForEach-Object { "$($_.ProcessName)#$($_.Id)" }) -join ', '
        Write-Log ("  警告：检测到其它 cargo/rustc 进程（{0}）。cargo 会等待 target 锁，构建可能变慢或报锁冲突，结束后重试即可。" -f $desc) 'Yellow'
    }

    Stop-ManagedDaozai '构建前' -ForeignPolicy warn

    if ($DryRun) {
        if ((Test-Path -LiteralPath $script:ExePath) -and (Test-ExeLocked $script:ExePath)) {
            Write-Log '  [DryRun] 当前 exe 被占用；真实运行会在停掉岛仔后再检查一次' 'Yellow'
        }
        Write-Log '  [DryRun] 将执行（仓库根）：. .\scripts\dev-msvc-env.ps1'
        Write-Log '  [DryRun] 将执行：cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc'
        Write-Log ("  [DryRun] 产物：{0}" -f $script:ExePath)
        $script:State.build = [ordered]@{
            planned = $true
            command = 'cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc'
            exePath = $script:ExePath
            sha256  = $null
            seconds = 0
        }
        return
    }

    if ((Test-Path -LiteralPath $script:ExePath) -and (Test-ExeLocked $script:ExePath)) {
        Start-Sleep -Seconds 1
        if (Test-ExeLocked $script:ExePath) {
            Fail 13 ("构建产物仍被占用：{0}`n" -f $script:ExePath +
                '可能被杀毒/索引/别的进程打开。用 handle.exe 或任务管理器查占用后再重跑。')
        }
    }

    # 加载 MSVC 环境（必须在仓库根；dev-msvc-env 会把 TEMP 指到 target\link-tmp）
    if (-not $script:MsvcEnvLoaded) {
        Write-Log '  加载 MSVC 构建环境（scripts\dev-msvc-env.ps1）'
        try {
            . $script:MsvcEnvScript
        } catch {
            Fail 10 ("加载 scripts\dev-msvc-env.ps1 失败：{0}`n" -f $_.Exception.Message +
                '常见原因：VS BuildTools + Windows SDK 未装齐、vcvars64.bat 找不到、脚本 BOM 丢失导致乱码。')
        }
        $cl = Get-Command 'cl.exe' -ErrorAction SilentlyContinue
        if (-not $cl) { Fail 10 'dev-msvc-env.ps1 执行后 PATH 里仍没有 cl.exe；检查 VS BuildTools 的 VCTools 组件。' }
        Write-Log ("  MSVC：{0}" -f $cl.Source)
        Write-Log ("  TEMP：{0}" -f $env:TEMP)
        $script:MsvcEnvLoaded = $true
    }

    $buildLog = Join-Path $script:RecordRunDir 'build.log'
    $result = Invoke-NativeLogged -Label 'cargo build' -FilePath 'cargo' `
        -Arguments @('build', '--locked', '--release', '--bin', 'daozai', '--target', 'x86_64-pc-windows-msvc') `
        -LogPath $buildLog

    if ($result.code -ne 0) {
        $joined = ($result.lines | ForEach-Object { "$_" }) -join "`n"
        $hint = ''
        if ($joined -match 'failed to remove file|Access is denied|os error 5|另一个程序正在使用此文件') {
            $hint = '看起来是 exe 被占用：先彻底退出岛仔（任务管理器 / Stop-Process -Name daozai -Force）再重跑 -Step Build。'
        } elseif ($joined -match 'LNK1104|LNK1181|link.exe') {
            $hint = '链接失败：确认已加载 scripts\dev-msvc-env.ps1，且 target\link-tmp 可写。'
        } elseif ($joined -match 'C1083|无法打开包括文件|fatal error C') {
            $hint = 'C 依赖编译失败：检查 VS BuildTools 的 VCTools + Windows SDK 是否装齐（缺 limits.h 等）。'
        }
        Write-Host ''
        Write-Log '构建输出尾部（最近 40 行）：' 'Yellow'
        foreach ($line in $result.tail) { Write-Host $line }
        $msg = ("cargo build 失败（退出码 {0}，耗时 {1}）。日志：{2}" -f $result.code, (Format-Seconds $result.seconds), $buildLog)
        if ($hint) { $msg += "`n提示：$hint" }
        Fail 14 $msg
    }

    Assert-File $script:ExePath '构建产物 daozai.exe' 14
    $item = Get-Item -LiteralPath $script:ExePath
    $sha = Get-Sha256 $script:ExePath
    $script:State.build = [ordered]@{
        planned       = $false
        command       = $result.command
        exePath       = $item.FullName
        sizeBytes     = $item.Length
        lastWriteTime = $item.LastWriteTime.ToString('s')
        sha256        = $sha
        seconds       = $result.seconds
        log           = $buildLog
    }
    Write-Log ("  构建成功：{0}" -f $item.FullName) 'Green'
    Write-Log ("  大小 {0:N0} 字节 | 时间 {1} | SHA256 {2}" -f $item.Length, $item.LastWriteTime, $sha)
}

function Invoke-VerifyStep {
    Assert-Tool 'cargo' '安装 Rust（rustup）或把 %USERPROFILE%\.cargo\bin 加进 PATH。' | Out-Null
    Assert-Tool 'node' '安装 Node.js（本机测试用 node --test）。' | Out-Null
    $nodeTest = Join-Path $script:RepoRoot 'tests\agent-notices.test.mjs'
    Assert-File $nodeTest 'tests\agent-notices.test.mjs'

    if (-not $script:MsvcEnvLoaded) {
        if ($DryRun) {
            Write-Log '  [DryRun] 将加载 MSVC 环境（cargo test --lib 需要链接器）'
        } else {
            Write-Log '  加载 MSVC 构建环境（cargo test --lib 需要链接器）'
            try { . $script:MsvcEnvScript } catch {
                Fail 10 ("加载 scripts\dev-msvc-env.ps1 失败：{0}" -f $_.Exception.Message)
            }
            if (-not (Get-Command 'cl.exe' -ErrorAction SilentlyContinue)) {
                Fail 10 'dev-msvc-env.ps1 执行后 PATH 里仍没有 cl.exe；检查 VS BuildTools 的 VCTools 组件。'
            }
            $script:MsvcEnvLoaded = $true
        }
    }

    $cargoCommands = @()
    if ($Fast) {
        $cargoCommands += , @('test', '--lib', '--', 'ui_tests')
        $cargoCommands += , @('test', '--lib', '--', 'agent')
    } else {
        $cargoCommands += , @('test', '--lib')
    }

    $cargoRuns = @()
    $allOk = $true
    $idx = 0
    foreach ($cmdArgs in $cargoCommands) {
        $idx++
        $logPath = Join-Path $script:RecordRunDir ("verify-cargo-{0}.log" -f $idx)
        if ($DryRun) {
            Write-Log ("  [DryRun] 将执行：cargo {0}" -f ($cmdArgs -join ' '))
            $cargoRuns += [ordered]@{
                command = ('cargo ' + ($cmdArgs -join ' '))
                code    = $null
                seconds = 0
                passed  = $null
                failed  = $null
                log     = $logPath
            }
            continue
        }
        $r = Invoke-NativeLogged -Label 'cargo test' -FilePath 'cargo' -Arguments $cmdArgs -LogPath $logPath
        $sum = Get-CargoTestSummary -Lines $r.lines
        if ($r.code -ne 0 -or -not $sum.ok) {
            $allOk = $false
            Write-Host ''
            Write-Log ("测试失败：cargo {0}（退出码 {1}）" -f ($cmdArgs -join ' '), $r.code) 'Red'
            foreach ($line in $r.tail) { Write-Host $line }
        }
        $cargoRuns += [ordered]@{
            command = $r.command
            code    = $r.code
            seconds = $r.seconds
            passed  = $sum.passed
            failed  = $sum.failed
            suites  = $sum.suites
            log     = $logPath
        }
        Write-Log ("  cargo {0}：退出码 {1}，passed {2} / failed {3}，耗时 {4}" -f ($cmdArgs -join ' '), $r.code, $sum.passed, $sum.failed, (Format-Seconds $r.seconds))
    }

    $nodeLog = Join-Path $script:RecordRunDir 'verify-node.log'
    $nodeRun = $null
    if ($DryRun) {
        Write-Log '  [DryRun] 将执行：node --test tests\agent-notices.test.mjs'
        $nodeRun = [ordered]@{ command = 'node --test tests\agent-notices.test.mjs'; code = $null; seconds = 0; pass = $null; fail = $null; log = $nodeLog }
    } else {
        $r = Invoke-NativeLogged -Label 'node test' -FilePath 'node' -Arguments @('--test', 'tests\agent-notices.test.mjs') -LogPath $nodeLog
        $sum = Get-NodeTestSummary -Lines $r.lines
        if ($r.code -ne 0) {
            $allOk = $false
            Write-Host ''
            Write-Log ("node 测试失败（退出码 {0}）" -f $r.code) 'Red'
            foreach ($line in $r.tail) { Write-Host $line }
        }
        $nodeRun = [ordered]@{
            command = $r.command
            code    = $r.code
            seconds = $r.seconds
            pass    = $sum.pass
            fail    = $sum.fail
            tests   = $sum.tests
            log     = $nodeLog
        }
        Write-Log ("  node --test：退出码 {0}，pass {1} / fail {2}，耗时 {3}" -f $r.code, $sum.pass, $sum.fail, (Format-Seconds $r.seconds))
    }

    $script:State.tests = [ordered]@{
        fast  = [bool]$Fast
        mode  = if ($Fast) { 'fast（ui_tests + agent + node）' } else { 'full（cargo test --lib + node）' }
        ok    = $allOk
        cargo = $cargoRuns
        node  = $nodeRun
    }

    if (-not $allOk -and -not $DryRun) {
        Fail 15 '测试未通过：看上面失败输出与 target\dev-cycle\<时间戳>\ 下的日志，修复后重跑 -Step Verify。'
    }
    if ($DryRun) {
        $script:State.tests.ok = $null
        Write-Log '  [DryRun] 测试命令已列出，未实际执行'
    } else {
        Write-Log '  测试全部通过' 'Green'
    }
}

function Invoke-E2EStep {
    Assert-DesktopUnlocked
    Assert-File $script:ExePath '构建产物 daozai.exe（先跑 -Step Build）' 21

    $reproScenarios = @('done-repro', 'jump-repro', 'scroll-repro', 'banner-jump-repro')
    if ($reproScenarios -contains $Scenario) {
        $e2eScript = Join-Path $script:RepoRoot 'scripts\agent-e2e-repro.ps1'
        $kind = 'repro'
    } else {
        $e2eScript = Join-Path $script:RepoRoot 'scripts\agent-e2e.ps1'
        $kind = 'standard'
    }
    Assert-File $e2eScript ('E2E 脚本 ' + (Split-Path -Leaf $e2eScript))
    $dshExe = Join-Path $env:LOCALAPPDATA 'Programs\DeepSeek Harness\DeepSeek Harness.exe'
    Assert-File $dshExe 'DeepSeek Harness 桌面端（E2E 宿主）'

    Stop-ManagedDaozai 'E2E 前（单实例互斥体会互踢）'

    $psExe = Get-PowerShellExe
    $e2eArgs = @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass',
        '-File', $e2eScript,
        '-Scenario', $Scenario,
        '-Exe', $script:ExePath,
        '-WaitSeconds', "$E2EWaitSeconds"
    )
    $shown = '{0} {1}' -f $psExe, ($e2eArgs -join ' ')

    if ($DryRun) {
        Write-Log ("  [DryRun] 将执行：{0}" -f $shown)
        Write-Log ("  [DryRun] 证据目录：{0}\<时间戳>" -f (Join-Path $script:RepoRoot 'docs\superpowers\research\agent-e2e-shots'))
        $script:State.e2e = [ordered]@{
            planned = $true
            kind    = $kind
            scenario = $Scenario
            command = $shown
            exitCode = $null
            ok       = $null
            shots    = $null
            summary  = $null
        }
        return
    }

    $outRoot = Join-Path $script:RepoRoot 'docs\superpowers\research\agent-e2e-shots'
    $logPath = Join-Path $script:RecordRunDir ("e2e-{0}.log" -f $Scenario)
    $startTime = Get-Date
    $r = Invoke-NativeLogged -Label 'agent-e2e' -FilePath $psExe -Arguments $e2eArgs -LogPath $logPath

    # 定位证据目录与 summary.json（比解析中文 stdout 稳）
    $shots = $null
    if (Test-Path -LiteralPath $outRoot) {
        $dirs = @(Get-ChildItem -LiteralPath $outRoot -Directory -ErrorAction SilentlyContinue |
                Where-Object { $_.LastWriteTime -ge $startTime.AddSeconds(-10) } |
                Sort-Object LastWriteTime -Descending)
        foreach ($d in $dirs) {
            if (Test-Path -LiteralPath (Join-Path $d.FullName 'summary.json')) { $shots = $d.FullName; break }
        }
    }
    if (-not $shots) {
        $joined = ($r.lines | ForEach-Object { "$_" }) -join "`n"
        $m = [regex]::Match($joined, '截图与结果[：:]\s*(.+)$', 'Multiline')
        if ($m.Success) { $shots = $m.Groups[1].Value.Trim() }
    }
    $summaryPath = $null
    $summary = $null
    if ($shots) {
        $summaryPath = Join-Path $shots 'summary.json'
        if (Test-Path -LiteralPath $summaryPath) {
            try { $summary = Get-Content -LiteralPath $summaryPath -Raw -Encoding UTF8 | ConvertFrom-Json } catch { $summary = $null }
        } else { $summaryPath = $null }
    }

    $script:State.e2e = [ordered]@{
        planned  = $false
        kind     = $kind
        scenario = $Scenario
        command  = $shown
        exitCode = $r.code
        ok       = ($r.code -eq 0)
        seconds  = $r.seconds
        shots    = $shots
        summary  = $summaryPath
        summaryOk = if ($summary) { $summary.ok } else { $null }
        failures = if ($summary) { @($summary.failures | Where-Object { $null -ne $_ }) } else { @() }
        log      = $logPath
    }

    if ($r.code -ne 0) {
        $reason = switch ($r.code) {
            2 { 'E2E 前置条件不满足（锁屏 / 缺少 DSH / OCR 不可用 / 缺 exe 等）' }
            3 { 'E2E 场景执行失败（卡片没弹出、点击后桥接没拿到预期决定等）' }
            4 { 'E2E 报「已有岛仔在运行」（单实例互斥体）——本步骤已停受管实例，若仍有外部实例请手动退出' }
            default { "E2E 脚本返回退出码 $($r.code)" }
        }
        Write-Host ''
        foreach ($line in $r.tail) { Write-Host $line }
        $msg = ("{0}。场景：{1}；日志：{2}" -f $reason, $Scenario, $logPath)
        if ($shots) { $msg += "`n证据目录：$shots" }
        if ($summaryPath) { $msg += "`nsummary：$summaryPath" }
        Fail 17 $msg
    }
    Write-Log ("  E2E 通过：场景 {0}（{1}），耗时 {2}" -f $Scenario, $kind, (Format-Seconds $r.seconds)) 'Green'
    if ($shots) { Write-Log ("  证据：{0}" -f $shots) }
}

function Invoke-DeliverStep {
    Assert-File $script:ExePath '构建产物 daozai.exe（先跑 -Step Build）' 21
    $logon = @(Get-Process -Name 'LogonUI' -ErrorAction SilentlyContinue)
    if ($logon.Count -gt 0) {
        Write-Log '  警告：当前工作站处于锁屏状态（LogonUI）。进程能启动、路径与哈希也能核对，但你看不到窗口；建议解锁后再交付。' 'Yellow'
    }
    $preHash = Get-Sha256 $script:ExePath
    if (-not $preHash) { Fail 16 ("读不到构建产物哈希：{0}" -f $script:ExePath) }
    $exeItem = Get-Item -LiteralPath $script:ExePath
    Write-Log ("  产物：{0}" -f $exeItem.FullName)
    Write-Log ("        {0:N0} 字节 | {1} | SHA256 {2}" -f $exeItem.Length, $exeItem.LastWriteTime, $preHash)
    if ($script:State.build -and $script:State.build.sha256 -and $script:State.build.sha256 -ne $preHash) {
        Fail 16 ("exe 与本次构建记录不一致：当前 {0}，构建记录 {1}。可能被别的构建覆盖，重跑 -Step Build。" -f $preHash, $script:State.build.sha256)
    }
    $shortcut = Get-DaozaiShortcut
    if ($shortcut) {
        Write-Log ("  桌面快捷方式：{0} → {1}" -f $shortcut.path, $shortcut.target)
        if ($shortcut.matches -eq $false) {
            Write-Log '  警告：快捷方式目标与本仓库构建产物不一致（可能是旧路径/旧机器），启动前请确认。' 'Yellow'
        }
    } else {
        Write-Log '  没找到指向 daozai.exe 的桌面快捷方式（不影响交付记录）' 'Yellow'
    }

    if ($DryRun) {
        Stop-ManagedDaozai '交付前'
        if ($NoStart) {
            Write-Log '  [DryRun] 按 -NoStart 只核对哈希/快捷方式，不启动'
        } else {
            Write-Log ("  [DryRun] 将启动：{0}" -f $script:ExePath)
            Write-Log '  [DryRun] 启动后核对：进程路径 == 构建产物、文件 SHA256 == 构建记录'
        }
        $script:State.deliver = [ordered]@{
            planned  = $true
            exePath  = $script:ExePath
            sha256   = $preHash
            started  = $false
            noStart  = [bool]$NoStart
            shortcut = $shortcut
        }
        return
    }

    Stop-ManagedDaozai '交付前'

    if ($NoStart) {
        Write-Log '  按 -NoStart 只核对哈希/快捷方式，不启动岛仔'
        $script:State.deliver = [ordered]@{
            planned  = $false
            exePath  = $script:ExePath
            sha256   = $preHash
            started  = $false
            noStart  = $true
            shortcut = $shortcut
        }
        return
    }

    Write-Log ("  启动：{0}" -f $script:ExePath)
    try {
        $p = Start-Process -FilePath $script:ExePath -WorkingDirectory $script:RepoRoot -PassThru
    } catch {
        Fail 16 ("启动失败：{0}" -f $_.Exception.Message)
    }
    $deadline = (Get-Date).AddSeconds($StartWaitSeconds)
    $proc = $null
    $procPath = $null
    while ((Get-Date) -lt $deadline) {
        $proc = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
        if (-not $proc) { break }
        try { $procPath = $proc.Path } catch { $procPath = $null }
        if ($procPath) { break }
        Start-Sleep -Milliseconds 250
    }
    if (-not $proc) {
        $exitCode = $null
        try { $exitCode = $p.ExitCode } catch { $exitCode = $null }
        Fail 16 ("新 exe 启动后立即退出（exit code：{0}）。先手动运行一次看是否有报错；也检查是否还有别的 daozai 实例占着单实例互斥体。" -f $exitCode)
    }
    if (-not $procPath) {
        Fail 16 '拿不到新进程的 Path（可能权限不足）；请用任务管理器确认进程路径后重试。'
    }
    if (-not ($procPath -ieq $script:ExePath)) {
        Fail 16 ("进程路径与构建产物不一致：{0}`n期望：{1}" -f $procPath, $script:ExePath)
    }
    Start-Sleep -Milliseconds 1500
    if (-not (Get-Process -Id $p.Id -ErrorAction SilentlyContinue)) {
        Fail 16 ("新 exe 启动后 {0} 秒内退出（可能崩溃或被互斥体踢掉）。" -f $StartWaitSeconds)
    }
    $postHash = Get-Sha256 $procPath
    if ($postHash -ne $preHash) {
        Fail 16 ("启动的 exe 哈希与构建产物不一致：进程 {0}，产物 {1}。可能有别的进程在启动期间替换了文件。" -f $postHash, $preHash)
    }
    $script:State.deliver = [ordered]@{
        planned       = $false
        exePath       = $script:ExePath
        sha256        = $postHash
        matches       = $true
        started       = $true
        noStart       = $false
        pid           = $p.Id
        processPath   = $procPath
        startedAt     = (Get-Date).ToString('s')
        shortcut      = $shortcut
    }
    Write-Log ("  交付校验通过：PID {0} | 路径 {1} | SHA256 {2}" -f $p.Id, $procPath, $postHash) 'Green'
}

function Invoke-CommitStep {
    Assert-Tool 'git' '安装 Git 并确保在 PATH。' | Out-Null
    $status = @(& git status --porcelain 2>&1)
    if ($LASTEXITCODE -ne 0) {
        Fail 18 ("git status 失败（不在仓库内或 .git 损坏）：{0}" -f (($status | ForEach-Object { "$_" }) -join ' '))
    }
    if ($status.Count -eq 0) {
        Write-Log '  工作区干净，没有可提交的改动'
        $script:State.commit = [ordered]@{ skipped = $true; reason = '工作区干净'; pushed = $false }
        return
    }

    $preview = @(& git add -A --dry-run 2>&1)
    $previewText = ($preview | ForEach-Object { "$_" }) -join "`n"
    if ($DryRun) {
        Write-Log '  [DryRun] git add -A 将暂存：'
        foreach ($line in $preview) { Write-Host ("    " + "$line") }
        $template = New-CommitMessage
        Write-Log ("  [DryRun] 将提交：{0}" -f $template.subject)
        Write-Log '  [DryRun] 提交正文：'
        foreach ($line in ($template.body -split "`n")) { Write-Host ("    " + $line) }
        Write-Log '  [DryRun] 不会执行 git commit，也绝不 push'
        $script:State.commit = [ordered]@{
            planned = $true
            subject = $template.subject
            body    = $template.body
            files   = $preview.Count
            pushed  = $false
        }
        return
    }

    Write-Log '  git add -A'
    $add = @(& git add -A 2>&1)
    if ($LASTEXITCODE -ne 0) {
        Fail 18 ("git add -A 失败：{0}" -f (($add | ForEach-Object { "$_" }) -join ' '))
    }
    $staged = @(& git diff --cached --name-status 2>&1)
    if ($staged.Count -eq 0) {
        Write-Log '  暂存区为空（改动可能已被忽略），跳过提交'
        $script:State.commit = [ordered]@{ skipped = $true; reason = '暂存区为空'; pushed = $false }
        return
    }
    Write-Log ("  暂存 {0} 个文件：" -f $staged.Count)
    foreach ($line in $staged) { Write-Host ("    " + "$line") }

    $template = New-CommitMessage
    Write-Log ("  提交：{0}" -f $template.subject)
    $commitOut = @(& git commit -m $template.subject -m $template.body 2>&1)
    if ($LASTEXITCODE -ne 0) {
        Fail 18 ("git commit 失败：{0}`n常见原因：user.name/user.email 未配置（git config --global user.name/user.email）、有未解决冲突、pre-commit hook 失败。" -f (($commitOut | ForEach-Object { "$_" }) -join "`n"))
    }
    $hash = ''
    try { $hash = ((& git rev-parse --short HEAD) | Out-String).Trim() } catch { $hash = '' }
    $script:State.commit = [ordered]@{
        skipped = $false
        subject = $template.subject
        body    = $template.body
        hash    = $hash
        files   = $staged.Count
        pushed  = $false
    }
    Write-Log ("  已提交 {0}（{1} 个文件）。按约定不 push。" -f $hash, $staged.Count) 'Green'
    Write-Log '  需要推送时手动：git push'
}

function New-CommitMessage {
    if ($Message -and $Message.Trim()) {
        $subject = $Message.Trim()
    } else {
        $subject = ('chore(dev-cycle): 本机测试版交付 {0}' -f $script:Stamp)
    }
    $lines = @()
    $lines += ('dev-cycle 自动提交（{0}，未 push）' -f $script:Stamp)
    if ($script:State.build -and $script:State.build.sha256) {
        $lines += ('EXE SHA256：{0}' -f $script:State.build.sha256)
        $lines += ('构建：{0} 字节，耗时 {1}s，产物 {2}' -f $script:State.build.sizeBytes, $script:State.build.seconds, $script:State.build.exePath)
    } elseif ($script:State.build -and $script:State.build.planned) {
        $lines += '构建：本次为 DryRun，未实际构建'
    }
    if ($script:State.tests) {
        $cargoDesc = @()
        foreach ($run in @($script:State.tests.cargo)) {
            if ($run.code -ne $null) {
                $cargoDesc += ('{0}（passed {1} / failed {2}）' -f $run.command, $run.passed, $run.failed)
            }
        }
        $nodeDesc = 'node --test tests\agent-notices.test.mjs'
        if ($script:State.tests.node -and $script:State.tests.node.code -ne $null) {
            $nodeDesc = ('node --test（pass {0} / fail {1}）' -f $script:State.tests.node.pass, $script:State.tests.node.fail)
        }
        $lines += ('测试：{0}；{1}' -f $script:State.tests.mode, (($cargoDesc + $nodeDesc) -join '；'))
    } else {
        $lines += '测试：本次未执行（可先跑 -Step Verify）'
    }
    if ($script:State.e2e) {
        if ($script:State.e2e.planned) {
            $lines += ('E2E：{0}（本次 DryRun 未执行）' -f $script:State.e2e.scenario)
        } else {
            $lines += ('E2E：{0} → {1}，证据 {2}' -f $script:State.e2e.scenario, ($(if ($script:State.e2e.ok) { '通过' } else { '失败' })), $script:State.e2e.shots)
        }
    }
    if ($script:State.deliver -and $script:State.deliver.started) {
        $lines += ('交付：PID {0}，SHA256 {1}，进程路径核对通过' -f $script:State.deliver.pid, $script:State.deliver.sha256)
    }
    $lines += ('运行记录：{0}' -f $script:RecordPath)
    return [pscustomobject]@{ subject = $subject; body = ($lines -join "`n") }
}

# ---------------------------------------------------------------------------
# 步骤解析与参数校验
# ---------------------------------------------------------------------------

function Resolve-StageList {
    $valid = @('SelfCheck', 'Build', 'Verify', 'Deliver', 'E2E', 'Commit', 'All')
    $requested = @()
    foreach ($raw in @($Step)) {
        foreach ($part in ([string]$raw -split '[,\s]+')) {
            $p = $part.Trim()
            if (-not $p) { continue }
            if ($valid -notcontains $p) {
                Fail 20 ("未知的 -Step 值：{0}。可选：{1}（多个步骤用逗号或空格分隔）。" -f $p, ($valid -join ', '))
            }
            $requested += $p
        }
    }
    if ($All) { $requested += 'All' }
    if ($requested.Count -eq 0) { $requested += 'SelfCheck' }

    $sawAll = ($requested -contains 'All')
    $wanted = @()
    foreach ($s in $requested) {
        if ($s -eq 'All') {
            foreach ($x in @('Build', 'Verify', 'E2E', 'Deliver')) {
                if ($wanted -notcontains $x) { $wanted += $x }
            }
        } else {
            if ($wanted -notcontains $s) { $wanted += $s }
        }
    }
    if ($sawAll -and ($Message -or $IncludeCommit) -and ($wanted -notcontains 'Commit')) {
        $wanted += 'Commit'
    }

    # 规范执行顺序：自检 → 构建 → 测试 → E2E → 交付 → 提交
    # E2E 需要独占单实例互斥体，会停掉正在跑的岛仔，所以放在交付之前，跑完桌面上留的是新版本。
    $canonical = @('SelfCheck', 'Build', 'Verify', 'E2E', 'Deliver', 'Commit')
    $ordered = @()
    foreach ($c in $canonical) {
        if ($wanted -contains $c) { $ordered += $c }
    }

    # 参数组合校验
    if ($Fast -and ($ordered -notcontains 'Verify')) {
        Fail 20 '-Fast 只影响 Verify 步骤，但本次没有 Verify（用 -Step Verify -Fast 或 -Step All -Fast）。'
    }
    if ($Message -and (-not $sawAll) -and ($ordered -notcontains 'Commit')) {
        Write-Log '  提示：给了 -Message 但没请求 Commit 步骤，本次不会提交。要提交请加 -Step Commit 或 -Step All。' 'Yellow'
    }
    if ($NoStart -and ($ordered -notcontains 'Deliver') -and ($ordered -notcontains 'E2E')) {
        Write-Log '  提示：-NoStart 只影响 Deliver，本次没有交付步骤。' 'Yellow'
    }
    if ($ordered.Count -eq 0) {
        Fail 20 '没有解析出任何步骤，请检查 -Step / -All 参数。'
    }
    return $ordered
}

# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------

$script:StartedAt = Get-Date
$script:ScriptPath = $PSCommandPath
if (-not $script:ScriptPath) { $script:ScriptPath = $MyInvocation.MyCommand.Path }
if ($script:ScriptPath) { $script:ScriptPath = [System.IO.Path]::GetFullPath($script:ScriptPath) }
$script:RecordSaved = $false
$script:State = $null
$script:CurrentStep = $null
$script:MsvcEnvLoaded = $false

$script:RepoRoot = Get-CycleRepoRoot
$script:ExePath = Join-Path $script:RepoRoot 'target\x86_64-pc-windows-msvc\release\daozai.exe'
$script:MsvcEnvScript = Join-Path $script:RepoRoot 'scripts\dev-msvc-env.ps1'
$script:Stamp = $script:StartedAt.ToString('yyyyMMdd-HHmmss')
$script:RecordDir = Join-Path $script:RepoRoot 'target\dev-cycle'
$script:RecordRunDir = Join-Path $script:RecordDir $script:Stamp
$script:RecordPath = Join-Path $script:RecordDir ($script:Stamp + '.json')
$script:LogPath = Join-Path $script:RecordDir ($script:Stamp + '.log')

$script:State = [ordered]@{
    schema      = 1
    tool        = 'scripts/dev-cycle.ps1'
    stamp       = $script:Stamp
    repoRoot    = $script:RepoRoot
    startedAt   = $script:StartedAt.ToString('s')
    finishedAt  = $null
    totalSeconds = 0
    host        = [ordered]@{
        psVersion = $PSVersionTable.PSVersion.ToString()
        edition   = "$($PSVersionTable.PSEdition)"
        os        = [Environment]::OSVersion.VersionString
        user      = "$env:USERDOMAIN\$env:USERNAME"
        dryRun    = [bool]$DryRun
    }
    params      = [ordered]@{
        step            = @($Step)
        all             = [bool]$All
        fast            = [bool]$Fast
        scenario        = $Scenario
        message         = $Message
        noStart         = [bool]$NoStart
        dryRun          = [bool]$DryRun
        includeCommit   = [bool]$IncludeCommit
        startWaitSeconds = $StartWaitSeconds
        e2eWaitSeconds  = $E2EWaitSeconds
    }
    skippedSteps = @()
    stages      = @()
    steps       = @()
    build       = $null
    tests       = $null
    deliver     = $null
    e2e         = $null
    commit      = $null
    exitCode    = 0
    error       = $null
    log         = @()
}

Push-Location -LiteralPath $script:RepoRoot
try {
    Write-Host ''
    Write-Host '岛仔开发循环 dev-cycle' -ForegroundColor Cyan
    Write-Host ("  仓库：{0}" -f $script:RepoRoot)
    Write-Host ("  PowerShell：{0}（{1}）" -f $PSVersionTable.PSVersion, $PSVersionTable.PSEdition)
    Write-Host ("  模式：{0}{1}" -f $(if ($DryRun) { 'DryRun（只看计划，不产生改动）' } else { '真实执行' }), $(if ($Fast) { ' / Fast' } else { '' }))

    # 自身 BOM 前置校验（BOM 丢失时 5.1 甚至无法解析，这里兜底给修复命令）
    $bom = Test-ScriptUtf8Bom
    if (-not $bom.ok) {
        $head = if ($bom.bytes) { ($bom.bytes | Select-Object -First 3 | ForEach-Object { '{0:X2}' -f $_ }) -join ' ' } else { '<读不到>' }
        Fail 19 ("本脚本缺少 UTF-8 BOM（前三位：$head）。Windows PowerShell 5.1 会按本地代码页解析中文并报错。`n" +
            "修复：`n  `$p='$($script:ScriptPath)'; `$t=[System.IO.File]::ReadAllText(`$p,[System.Text.Encoding]::UTF8)`n" +
            "  [System.IO.File]::WriteAllText(`$p,`$t,(New-Object System.Text.UTF8Encoding(`$true)))")
    }

    $stages = @(Resolve-StageList)
    $script:State.stages = $stages
    Write-Host ("  步骤：{0}" -f ($stages -join ' → '))
    New-Item -ItemType Directory -Force -Path $script:RecordRunDir | Out-Null

    foreach ($stage in $stages) {
        switch ($stage) {
            'SelfCheck' { Invoke-CycleStep 'SelfCheck' { Invoke-SelfCheckStep } }
            'Build' { Invoke-CycleStep 'Build' { Invoke-BuildStep } }
            'Verify' { Invoke-CycleStep 'Verify' { Invoke-VerifyStep } }
            'E2E' { Invoke-CycleStep 'E2E' { Invoke-E2EStep } }
            'Deliver' { Invoke-CycleStep 'Deliver' { Invoke-DeliverStep } }
            'Commit' { Invoke-CycleStep 'Commit' { Invoke-CommitStep } }
            default { Fail 20 ("未知步骤：{0}" -f $stage) }
        }
    }

    # 汇总
    Write-Host ''
    Write-Host '================ 汇总 ================' -ForegroundColor Cyan
    foreach ($s in $script:State.steps) {
        $statusText = $s.status
        $color = 'White'
        if ($s.status -eq 'ok') { $color = 'Green' }
        elseif ($s.status -eq 'dry-run') { $color = 'DarkGray' }
        elseif ($s.status -eq 'failed') { $color = 'Red' }
        Write-Host ("  {0,-10} {1,-8} {2,8}  {3}" -f $s.name, $statusText, (Format-Seconds $s.seconds), $s.detail) -ForegroundColor $color
    }
    if ($script:State.build -and $script:State.build.sha256) {
        Write-Host ("  exe：{0}" -f $script:ExePath)
        Write-Host ("       SHA256 {0} | {1:N0} 字节 | {2}" -f $script:State.build.sha256, $script:State.build.sizeBytes, $script:State.build.lastWriteTime)
    }
    if ($script:State.tests) {
        Write-Host ("  测试：{0}" -f $script:State.tests.mode)
        foreach ($run in @($script:State.tests.cargo)) {
            if ($null -ne $run.code) {
                Write-Host ("    {0} → 退出码 {1}，passed {2} / failed {3}，{4}" -f $run.command, $run.code, $run.passed, $run.failed, (Format-Seconds $run.seconds))
            }
        }
        if ($script:State.tests.node -and $null -ne $script:State.tests.node.code) {
            Write-Host ("    {0} → 退出码 {1}，pass {2} / fail {3}，{4}" -f $script:State.tests.node.command, $script:State.tests.node.code, $script:State.tests.node.pass, $script:State.tests.node.fail, (Format-Seconds $script:State.tests.node.seconds))
        }
    }
    if ($script:State.e2e -and -not $script:State.e2e.planned) {
        Write-Host ("  E2E：{0} → {1}（证据 {2}）" -f $script:State.e2e.scenario, $(if ($script:State.e2e.ok) { '通过' } else { '失败' }), $script:State.e2e.shots)
    }
    if ($script:State.commit -and -not $script:State.commit.skipped -and -not $script:State.commit.planned) {
        Write-Host ("  提交：{0}（{1} 个文件，未 push）" -f $script:State.commit.hash, $script:State.commit.files)
    }
    if ($DryRun) { Write-Host '  本次为 DryRun：没有构建、没有启停岛仔、没有跑测试、没有提交。' -ForegroundColor Yellow }
    Write-Host ''

    Save-CycleRecord 0 ''
    exit 0
} finally {
    Pop-Location
}
