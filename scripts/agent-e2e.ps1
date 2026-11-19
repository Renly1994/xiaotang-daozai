<#
岛仔 Agent 界面 E2E 验收：真实 DSH 宿主 → 岛仔界面 → 屏幕识别点击 → 桥接侧校验。

流程（每个场景都跑这三段）：
  1) 用已安装的 DeepSeek Harness（ELECTRON_RUN_AS_NODE=1）跑 tests/fixtures/agent-drive.cjs，
     它加载临时 profile、挂真实 dsh-session/dsh-agent/dsh-user-approval/dsh-user-questions
     与岛仔 agent-notices bundle，通过命名管道向岛仔发出「审批 / 提问 / 完成」请求；
  2) 桌面上岛仔弹出卡片后，本脚本用 scripts/agent-screen-ocr.ps1 截图 + Windows OCR 定位按钮，
     用 SetCursorPos + mouse_event 真点（不是 DOM 注入，也不是伪造桥接回复）；
  3) 岛仔把决定写回桥接，驱动打印 JSON；本脚本校验 allow/deny/answer/done 是否与屏幕上点的按钮一致，
     并把每一步的截图和 OCR 结果留在 docs/superpowers/research/agent-e2e-shots/<时间戳>/ 下。

用法（在仓库根目录；PowerShell 5.1 或 7 都行）：

  # 全场景（approval + question + done），用默认 release exe
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1

  # 只跑审批，用指定 exe 和独立数据目录
  ... -Scenario approval -Exe target\x86_64-pc-windows-msvc\release\daozai.exe -DataDir %TEMP%\daozai-e2e-run

  # 跑完保留数据目录与岛仔进程，便于人工看现场
  ... -KeepAlive

  # 只验证「卡片能弹出来 + OCR 能读到按钮」，不点击（给 UI 迭代用）
  ... -Scenario all -NoClick

前置条件：
  * 已安装 DeepSeek Harness 桌面端（%LOCALAPPDATA%\Programs\DeepSeek Harness\DeepSeek Harness.exe）；
  * 没有其它 daozai 实例在跑（单实例互斥体 Local\Daozai.SingleInstance，会互踢）；
  * 系统有中文 OCR 语言包（Win10/11 自带；脚本会先自检）。

退出码：0 全部通过；2 前置条件不满足；3 场景失败（超时/点击后没有拿到预期决定）；4 已有 daozai 在运行。

注意：本文件必须以 UTF-8 with BOM 保存，否则 Windows PowerShell 5.1 会按本地代码页解析中文而报错。
#>
[CmdletBinding()]
param(
  [string]$Exe,
  [ValidateSet('approval', 'question', 'question-multi', 'question-multi-turn', 'done', 'all')]
  [string]$Scenario = 'all',
  [string]$DataDir,
  [int]$WaitSeconds = 90,
  [string]$OutRoot,
  [switch]$KeepAlive,
  [switch]$NoClick
)

$ErrorActionPreference = 'Stop'
$script:RepoRoot = Split-Path -Parent $PSScriptRoot
$script:Failures = @()
$script:Results = @()
$script:Log = @()
$script:Shots = $null
$script:LastRegion = $null

function Write-Step([string]$message) {
  $line = '[{0}] {1}' -f (Get-Date -Format 'HH:mm:ss'), $message
  $script:Log += $line
  Write-Host $line
}
function Save-Summary($payload) {
  if ($script:Shots) {
    $path = Join-Path $script:Shots 'summary.json'
    [System.IO.File]::WriteAllText($path, ($payload | ConvertTo-Json -Depth 10), (New-Object System.Text.UTF8Encoding($false)))
  }
}
function Fail([int]$code, [string]$message) {
  Write-Host "失败：$message" -ForegroundColor Red
  Save-Summary ([pscustomobject]@{ ok = $false; code = $code; error = $message; log = $script:Log; results = $script:Results })
  exit $code
}

Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Threading;
public static class DaozaiE2EInput {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr param);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr param);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  // 按进程 PID 找岛仔主窗口：比按标题精确匹配稳（标题含中文，跨进程代码页容易踩坑）。
  public static IntPtr FindWindowForProcess(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr hwnd, IntPtr param) {
      uint owner;
      GetWindowThreadProcessId(hwnd, out owner);
      if (owner != pid || !IsWindowVisible(hwnd)) return true;
      RECT rect;
      if (!GetWindowRect(hwnd, out rect)) return true;
      if (rect.Right - rect.Left < 120 || rect.Bottom - rect.Top < 20) return true;
      found = hwnd;
      return false;
    }, IntPtr.Zero);
    return found;
  }
  public static bool Click(int x, int y) {
    if (!SetCursorPos(x, y)) return false;
    Thread.Sleep(60);
    mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); // left down
    Thread.Sleep(40);
    mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero); // left up
    Thread.Sleep(80);
    return true;
  }
}
'@

# ---------------------------------------------------------------- 前置检查

$dshExe = Join-Path $env:LOCALAPPDATA 'Programs\DeepSeek Harness\DeepSeek Harness.exe'
if (-not (Test-Path $dshExe)) { Fail 2 "找不到 DeepSeek Harness：$dshExe" }
Write-Step "DSH 宿主：$dshExe"

if (-not $Exe) { $Exe = Join-Path $script:RepoRoot 'target\x86_64-pc-windows-msvc\release\daozai.exe' }
if (-not (Test-Path $Exe)) {
  Fail 2 ("找不到岛仔 exe：$Exe`n先构建：powershell -NoProfile -Command "". .\scripts\dev-msvc-env.ps1; cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc""")
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
Write-Step "岛仔 exe：$Exe（$((Get-Item $Exe).LastWriteTime)）"

$other = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -like 'daozai*' })
if ($other.Count) {
  Fail 4 ("已有岛仔在运行（PID $(($other | ForEach-Object { $_.Id }) -join ', ')），单实例互斥体会互踢。`n" +
    '请先退出现有实例（Stop-Process -Id <pid>）再跑 E2E；脚本不会动别人的岛仔。')
}

$ocrScript = Join-Path $PSScriptRoot 'agent-screen-ocr.ps1'
$driveScript = Join-Path $script:RepoRoot 'tests\fixtures\agent-drive.cjs'
foreach ($f in @($ocrScript, $driveScript)) { if (-not (Test-Path $f)) { Fail 2 "缺少文件：$f" } }

# OCR 自检：能创建引擎、能跑通一次识别
$ocrProbeRaw = & powershell -NoProfile -ExecutionPolicy Bypass -File $ocrScript -Region '0,0,64,64' -Scale 1 2>&1 | Out-String
try { $ocrProbe = $ocrProbeRaw | ConvertFrom-Json } catch { Fail 2 "OCR 自检输出无法解析：$($ocrProbeRaw.Substring(0, [Math]::Min(300, $ocrProbeRaw.Length)))" }
if (-not $ocrProbe.ok) { Fail 2 "OCR 自检失败：$($ocrProbe.error)" }
Write-Step "OCR 可用：语言 $($ocrProbe.language)"

# ---------------------------------------------------------------- 独立数据目录与截图目录

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if (-not $DataDir) { $DataDir = Join-Path $env:TEMP "daozai-e2e-$stamp" }
$DataDir = [System.IO.Path]::GetFullPath($DataDir)
if (-not $OutRoot) { $OutRoot = Join-Path $script:RepoRoot 'docs\superpowers\research\agent-e2e-shots' }
$script:Shots = Join-Path $OutRoot $stamp
New-Item -ItemType Directory -Force -Path $DataDir, $script:Shots, (Join-Path $DataDir 'dsh-home\profiles\desktop'), (Join-Path $DataDir 'codex-home') | Out-Null
Write-Step "数据目录：$DataDir"
Write-Step "截图目录：$($script:Shots)"
[System.IO.File]::WriteAllText((Join-Path $DataDir 'dsh-home\profiles\desktop\package.json'), '{"private":true}', (New-Object System.Text.UTF8Encoding($false)))

# 跑副本：不锁 release 构建产物，进程名也区分开（daozai-e2e）。
$runExe = Join-Path $DataDir 'daozai-e2e.exe'
Copy-Item -LiteralPath $Exe -Destination $runExe -Force
Write-Step "运行副本：$runExe"

# 设置：只打开 DSH 来源；岛上处理审批/提问；完成通知=本轮回复+展开结果卡；不自动交回。
# island_size=140 是给 OCR 用的显示缩放（100% 下 Windows OCR 认不出 11px 中文按钮，见验收文档）。
$agentPrefs = '{"interactive":true,"handoff":0,"completion":true,"dwell":15,"expanded":true,"content":2,"deferred":true,"system_toast":false}'
$settings = @(
  'notifications=1', 'system_notify=0', 'sound=0', 'read_notifications=0',
  'dsh_notifications=1', 'codex_notifications=0',
  "agent_cards=$agentPrefs",
  'hide_fullscreen=0', 'avoid_top_bar=0', 'pet=0', 'island_size=140'
)
[System.IO.File]::WriteAllLines((Join-Path $DataDir 'settings.txt'), $settings, (New-Object System.Text.UTF8Encoding($false)))

# ---------------------------------------------------------------- 工具函数

function Get-ScreenBounds {
  $v = [System.Windows.Forms.SystemInformation]::VirtualScreen
  [pscustomobject]@{ left = $v.Left; top = $v.Top; right = $v.Right; bottom = $v.Bottom }
}

# 岛仔窗口比“岛体”宽：窗口画布 CANVAS_W(960) 逻辑宽，岛体只有 WIN_W(640)，多出来的是透明区。
# 卡片水平居中在“屏幕中心”上（实测 3440 屏、island_size=140 时卡片 1432..1992，中心 1712≈1720），
# 所以只取「屏幕中心 ±（419×scale/2 + 40）」这条竖带：既覆盖卡片，又不把桌面其它窗口带进截图。
$script:UiScale = 1.4              # 与 settings.txt 的 island_size=140 对应
$script:CardWidthPx = 419 * $script:UiScale

function Get-IslandRect {
  if (-not $script:islandPid) { return $null }
  $hwnd = [DaozaiE2EInput]::FindWindowForProcess([uint32]$script:islandPid)
  if ($hwnd -eq [IntPtr]::Zero) { return $null }
  $rect = New-Object DaozaiE2EInput+RECT
  if (-not [DaozaiE2EInput]::GetWindowRect($hwnd, [ref]$rect)) { return $null }
  if (($rect.Right - $rect.Left) -le 0 -or ($rect.Bottom - $rect.Top) -le 0) { return $null }
  $screen = Get-ScreenBounds
  $cx = [int](($screen.left + $screen.right) / 2)
  $half = [int][Math]::Ceiling($script:CardWidthPx / 2) + 70
  $bottom = [Math]::Min([Math]::Max($rect.Bottom, 260), 640)
  [pscustomobject]@{
    left = [Math]::Max($screen.left, $cx - $half)
    right = [Math]::Min($screen.right, $cx + $half)
    top = [Math]::Max($screen.top, $rect.Top - 8)
    bottom = $bottom
  }
}

# 等到岛仔窗口出现（进程刚启动时窗口还没建好）。
function Wait-IslandRect([int]$TimeoutSeconds = 15) {
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    $rect = Get-IslandRect
    if ($rect) { return $rect }
    Start-Sleep -Milliseconds 200
  }
  return $null
}
function Get-RegionString($region) { "$($region.left),$($region.top),$($region.right),$($region.bottom)" }

# 卡片出现/展开会让岛仔窗口变大；每轮都重新取一次窗口矩形，避免只截到收起态的小窗口。
# 取不到（窗口正在重建/瞬间不可见）时退回上一次可用的矩形，再退回屏幕中心竖带。
function Get-LiveRegion($fallback) {
  if ($script:islandPid) {
    $fresh = Get-IslandRect
    if ($fresh) { $script:LastRegion = $fresh; return $fresh }
  }
  if ($script:LastRegion) { return $script:LastRegion }
  if ($fallback) { return $fallback }
  $screen = Get-ScreenBounds
  $cx = [int](($screen.left + $screen.right) / 2)
  $half = [int][Math]::Ceiling($script:CardWidthPx / 2) + 70
  [pscustomobject]@{ left = $cx - $half; right = $cx + $half; top = $screen.top; bottom = [Math]::Min(640, $screen.bottom) }
}

function Invoke-Ocr {
  param($Region, [string]$Find, [string]$SaveImage, [int[]]$Scales = @(2, 3, 4, 6))
  $Region = Get-LiveRegion $Region
  $cli = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $ocrScript, '-Region', (Get-RegionString $Region), '-FindScales', ($Scales -join ','))
  if ($Find) { $cli += @('-Find', $Find) }
  if ($SaveImage) { $cli += @('-SaveImage', $SaveImage, '-CropSave', '-CropCard') }
  # OCR 子进程失败（参数/编码/引擎）只应让本次识别返回 null，不能把整个验收脚本打断。
  $previous = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try { $raw = & powershell @cli 2>&1 | Out-String }
  finally { $ErrorActionPreference = $previous }
  try { return ($raw | ConvertFrom-Json) } catch {
    return [pscustomobject]@{ ok = $false; error = "OCR 输出无法解析: $($raw.Substring(0, [Math]::Min(200, $raw.Length)))" }
  }
}

# 轮询直到任意一个候选文字出现；返回 @{found;center;text;line} 或 $null。
function Wait-Text {
  param($Region, [string[]]$Texts, [int]$TimeoutSeconds, [string]$ShotOnFound, [string]$ShotOnTimeout)
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $attempt = 0
  while ((Get-Date) -lt $deadline) {
    $attempt++
    $live = Get-LiveRegion $Region
    foreach ($text in $Texts) {
      $shot = $null
      if ($ShotOnFound -and $attempt -eq 1) { $shot = $ShotOnFound }
      $j = Invoke-Ocr -Region $live -Find $text -SaveImage $shot
      if ($j.ok -and $j.find -and $j.find.found) {
        if ($ShotOnFound -and $attempt -gt 1) { $null = Invoke-Ocr -Region $live -Find $text -SaveImage $ShotOnFound }
        return [pscustomobject]@{ found = $true; center = $j.find.center; text = $text; line = $j.find.line; attempts = $attempt }
      }
    }
    Start-Sleep -Milliseconds 400
  }
  if ($ShotOnTimeout) { $null = Invoke-Ocr -Region (Get-LiveRegion $Region) -SaveImage $ShotOnTimeout }
  return $null
}

function Find-AnyText {
  param($Region, [string[]]$Texts, [string]$SaveImage, [int[]]$Scales = @(2, 3, 4, 6), [switch]$PreferBottom)
  $live = Get-LiveRegion $Region
  foreach ($text in $Texts) {
    $j = Invoke-Ocr -Region $live -Find $text -SaveImage $SaveImage -Scales $Scales
    if ($j.ok -and $j.find -and $j.find.found) {
      # 同一个词可能同时出现在题干和选项里：选项一定在题干下方，取命中里最靠下的一条。
      $center = $j.find.center
      $hits = @($j.find.matches)
      if ($PreferBottom -and $hits.Count -gt 1) {
        $center = ($hits | Sort-Object -Property @{ Expression = { $_.center[1] } } | Select-Object -Last 1).center
      }
      return [pscustomobject]@{ found = $true; center = $center; text = $text; line = $j.find.line; matches = $hits }
    }
  }
  return $null
}

function Click-Point($center) {
  $x = [int]$center[0]; $y = [int]$center[1]
  if ($NoClick) { Write-Step "  （-NoClick）本应点击 ($x,$y)"; return $true }
  $ok = [DaozaiE2EInput]::Click($x, $y)
  Write-Step "  点击 ($x,$y) -> $ok"
  Start-Sleep -Milliseconds 500
  return $ok
}

function Wait-DriverExit([int]$DriverPid, [int]$TimeoutSeconds) {
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    if (-not (Get-Process -Id $DriverPid -ErrorAction SilentlyContinue)) { return $true }
    Start-Sleep -Milliseconds 250
  }
  return $false
}

function Read-DriverJson([string]$StdoutFile) {
  if (-not (Test-Path $StdoutFile)) { return $null }
  try {
    # 必须显式按 UTF-8 读：Windows PowerShell 5.1 的 Get-Content -Raw 默认按 ANSI 解码，
    # 会把驱动输出里的中文读到乱码，进而让 ConvertFrom-Json 失败（第一版就栽在这里）。
    $text = [System.IO.File]::ReadAllText($StdoutFile, (New-Object System.Text.UTF8Encoding($false)))
  } catch {
    return $null  # 重定向文件可能正被写入，交给调用方重试
  }
  if (-not $text) { return $null }
  $line = ($text.Trim() -split "`r?`n" | Where-Object { $_.TrimStart().StartsWith('{') } | Select-Object -Last 1)
  if (-not $line) { return $null }
  try { return ($line | ConvertFrom-Json) } catch { return $null }
}

# ---------------------------------------------------------------- 岛仔进程

$script:islandPid = $null
$envBackup = @{}
function Set-EnvVar([string]$name, $value) {
  if (-not $envBackup.ContainsKey($name)) { $envBackup[$name] = [System.Environment]::GetEnvironmentVariable($name, 'Process') }
  [System.Environment]::SetEnvironmentVariable($name, $value, 'Process')
}
function Restore-Env {
  foreach ($key in $envBackup.Keys) { [System.Environment]::SetEnvironmentVariable($key, $envBackup[$key], 'Process') }
}

function Start-Island {
  Set-EnvVar 'DAOZAI_DATA_DIR' $DataDir
  Set-EnvVar 'DSH_HOME' (Join-Path $DataDir 'dsh-home')
  Set-EnvVar 'CODEX_HOME' (Join-Path $DataDir 'codex-home')
  $p = Start-Process -FilePath $runExe -PassThru
  $script:islandPid = $p.Id
  Write-Step "启动岛仔 PID=$($script:islandPid)"
  $live = Join-Path $DataDir 'agent-notices\bridge\live.json'
  $deadline = (Get-Date).AddSeconds(30)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path $live) {
      try {
        $entry = Get-Content -Raw -LiteralPath $live | ConvertFrom-Json
        if ($entry.policy.dsh) { Write-Step "桥接就绪：$live"; return $true }
      } catch { }
    }
    if (-not (Get-Process -Id $script:islandPid -ErrorAction SilentlyContinue)) { return $false }
    Start-Sleep -Milliseconds 200
  }
  return $false
}

function Stop-Island {
  if ($script:islandPid) {
    $p = Get-Process -Id $script:islandPid -ErrorAction SilentlyContinue
    if ($p) { Stop-Process -Id $script:islandPid -Force -ErrorAction SilentlyContinue; Write-Step "已停止岛仔 PID=$($script:islandPid)" }
    $script:islandPid = $null
  }
  Start-Sleep -Milliseconds 600
}

# ---------------------------------------------------------------- 场景参数

# 场景名 -> 驱动场景 + 屏幕侧动作参数
function Get-ScenarioSpec([string]$name) {
  switch ($name) {
    'approval' {
      return [pscustomobject]@{
        driver = 'approval'
        request = [ordered]@{
          tool = 'functions.bash'
          operation = "{`n  `"command`": `"git push --force origin main`",`n  `"timeout`": 30000`n}"
          reason = '岛仔端到端验收：请在岛上点击「仅允许这一次」'
        }
        options = @()
      }
    }
    'question' {
      # 选项用 OCR 友好的短词：Windows OCR 会把「项」认成「顶」、「查」认成「鱼」，
      # 所以测试数据不用「甲选项」这类易混字，避免把测试数据的问题当成 UI 问题。
      return [pscustomobject]@{ driver = 'question-single'; request = [ordered]@{ options = @('冒烟测试', '完整回归', '先不验证') }; options = @('冒烟测试') }
    }
    'question-multi' {
      return [pscustomobject]@{ driver = 'question-multi'; request = [ordered]@{ options = @('行高', '进度', '滚动') }; options = @('行高', '滚动') }
    }
    'question-multi-turn' {
      return [pscustomobject]@{ driver = 'question-multi-turn'; request = [ordered]@{ options = @('方案一', '方案二') }; options = @('方案一') }
    }
    'done' {
      return [pscustomobject]@{
        driver = 'done'
        request = [ordered]@{ text = "E2E 完成正文：岛仔端到端验收。`n- 标记 daozai-e2e-done`n- 如果你在岛上看到这段话，说明真实 DSH 宿主 → 桥接 → 岛仔的完成链路是通的。" }
        options = @()
      }
    }
    default { return $null }
  }
}

# ---------------------------------------------------------------- 主流程

$scenarios = if ($Scenario -eq 'all') { @('approval', 'question', 'done') } else { @($Scenario) }

try {
  if (-not (Start-Island)) {
    Fail 3 '岛仔没有在 30 秒内发布桥接通道（检查设置里 DSH 来源是否打开、exe 是否能正常启动）'
  }
  Start-Sleep -Milliseconds 800

  Set-EnvVar 'ELECTRON_RUN_AS_NODE' '1'
  Set-EnvVar 'DSH_HOME' (Join-Path $DataDir 'dsh-home')
  $profileDir = Join-Path $DataDir 'dsh-home\profiles\desktop'

  foreach ($name in $scenarios) {
    $spec = Get-ScenarioSpec $name
    if (-not $spec) { Fail 2 "未知场景 $name" }
    $scenarioStart = Get-Date
    Write-Step "===== 场景 $name（驱动场景 $($spec.driver)）====="
    $shotBase = Join-Path $script:Shots $name
    $stdout = Join-Path $script:Shots "$name-driver.json"
    $stderr = Join-Path $script:Shots "$name-driver.err.log"
    $jsonIn = Join-Path $script:Shots "$name-driver-input.json"
    [System.IO.File]::WriteAllText($jsonIn, ($spec.request | ConvertTo-Json -Depth 6), (New-Object System.Text.UTF8Encoding($false)))

    $driverArgs = @($driveScript, '--profile', $profileDir, '--scenario', $spec.driver,
      '--wait', [string]($WaitSeconds * 1000), '--json-in', $jsonIn)
    $driver = Start-Process -FilePath $dshExe -ArgumentList $driverArgs -PassThru -WorkingDirectory $script:RepoRoot `
      -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    Write-Step "  驱动 PID=$($driver.Id)"

    $verdict = [pscustomobject]@{ scenario = $name; ok = $false; steps = @(); driver = $null; error = $null; bridgeOk = $false; duration = 0 }

    $region = Wait-IslandRect 15
    if (-not $region) {
      $verdict.error = '15 秒内没有找到岛仔窗口（进程是否已退出？单实例互斥体被占用时它会激活旧实例后退出）'
    }
    elseif ($NoClick) {
      # 只做“看得见”的检查：卡片是否弹出、关键按钮文字能否被 OCR 读到（不点击、不等决定）。
      $keys = switch ($name) {
        'approval' { @('等待你审批', '查看并审批', '仅允许这一次') }
        'done' { @('本轮已结束', '本轮已', '查看回复', '本轮回复') }
        default { @('需要你回答', '检查答案', '提交回答', '冒烟测试', '行高', '方案一') }
      }
      $seen = @()
      $hit = Wait-Text -Region $region -Texts ($keys | Select-Object -First 1) -TimeoutSeconds 25 -ShotOnFound "$shotBase-01-visible.png" -ShotOnTimeout "$shotBase-01-timeout.png"
      if (-not $hit) { $verdict.error = '25 秒内没有在岛上看到卡片' }
      else {
        foreach ($key in $keys) {
          $found = Find-AnyText -Region $region -Texts @($key) -SaveImage "$shotBase-vision-$key.png"
          if ($found) { $seen += $key }
        }
        $verdict.steps += [pscustomobject]@{ step = 'OCR 可见按钮'; ok = $true; text = ($seen -join ' / ') }
        $verdict.ok = $true
      }
      Stop-Process -Id $driver.Id -Force -ErrorAction SilentlyContinue
    }
    elseif ($name -eq 'approval') {
      $hit = Wait-Text -Region $region -Texts @('等待你审批', '等待审批') -TimeoutSeconds 25 -ShotOnFound "$shotBase-01-banner.png" -ShotOnTimeout "$shotBase-01-timeout.png"
      $verdict.steps += [pscustomobject]@{ step = '等到审批卡片'; ok = [bool]$hit; text = $(if ($hit) { $hit.text } else { $null }) }
      if (-not $hit) { $verdict.error = '25 秒内没有在岛上看到审批卡片' }
      else {
        # 卡片可能直接展开成详情，也可能先是收起态（需要点「查看并审批」）。
        $region = Get-IslandRect
        $detail = Find-AnyText -Region $region -Texts @('仅允许这一次', '允许这一次') -SaveImage "$shotBase-02-detail.png" -PreferBottom
        if (-not $detail) {
          $review = Find-AnyText -Region $region -Texts @('查看并审批', '查看并', '查看') -SaveImage "$shotBase-02-review.png"
          $verdict.steps += [pscustomobject]@{ step = '打开审批详情'; ok = [bool]$review; text = $(if ($review) { $review.text } else { $null }) }
          if ($review) {
            $null = Click-Point $review.center
            Start-Sleep -Milliseconds 900
            $region = Get-IslandRect
            $detail = Wait-Text -Region $region -Texts @('仅允许这一次', '允许这一次') -TimeoutSeconds 12 -ShotOnFound "$shotBase-03-detail.png" -ShotOnTimeout "$shotBase-03-timeout.png"
          }
        } else {
          $verdict.steps += [pscustomobject]@{ step = '详情已自动展开'; ok = $true; text = $detail.text }
        }
        $verdict.steps += [pscustomobject]@{ step = '看到「仅允许这一次」'; ok = [bool]$detail; text = $(if ($detail) { $detail.text } else { $null }) }
        # 顺带确认「操作内容」真的显示在卡片上（OCR 可能读花，非致命）。
        $opSeen = Find-AnyText -Region $region -Texts @('command', 'git push') -SaveImage "$shotBase-02-operation.png"
        $verdict.steps += [pscustomobject]@{ step = '审批正文包含操作内容'; ok = [bool]$opSeen; text = $(if ($opSeen) { $opSeen.line } else { $null }); fatal = $false }
        if (-not $detail) { $verdict.error = '审批详情里没有识别到「仅允许这一次」按钮' }
        else {
          $null = Click-Point $detail.center
          $verdict.steps += [pscustomobject]@{ step = '点击允许'; ok = $true; center = $detail.center }
        }
      }
    }
    elseif ($name -like 'question*') {
      $hit = Wait-Text -Region $region -Texts @('需要你回答', '需要回答') -TimeoutSeconds 25 -ShotOnFound "$shotBase-01-question.png" -ShotOnTimeout "$shotBase-01-timeout.png"
      $verdict.steps += [pscustomobject]@{ step = '等到提问卡片'; ok = [bool]$hit; text = $(if ($hit) { $hit.text } else { $null }) }
      if (-not $hit) { $verdict.error = '25 秒内没有在岛上看到提问卡片' }
      else {
        $okOptions = $true
        foreach ($opt in $spec.options) {
          $region = Get-IslandRect
          $hitOpt = Find-AnyText -Region $region -Texts @($opt) -SaveImage "$shotBase-02-option-$opt.png" -PreferBottom
          $verdict.steps += [pscustomobject]@{ step = "点击选项「$opt」"; ok = [bool]$hitOpt; text = $(if ($hitOpt) { $hitOpt.text } else { $null }) }
          if (-not $hitOpt) { $okOptions = $false; break }
          $null = Click-Point $hitOpt.center
        }

        if ($okOptions -and $name -eq 'question-multi-turn') {
          $region = Get-IslandRect
          $next = Find-AnyText -Region $region -Texts @('下一题') -SaveImage "$shotBase-03-next.png"
          $verdict.steps += [pscustomobject]@{ step = '点击「下一题」'; ok = [bool]$next; text = $(if ($next) { $next.text } else { $null }) }
          if ($next) {
            $null = Click-Point $next.center
            Start-Sleep -Milliseconds 700
            $region = Get-IslandRect
            $hit2 = Find-AnyText -Region $region -Texts @('确认一') -SaveImage "$shotBase-04-option2.png" -PreferBottom
            $verdict.steps += [pscustomobject]@{ step = '点击第 2 题选项'; ok = [bool]$hit2; text = $(if ($hit2) { $hit2.text } else { $null }) }
            if ($hit2) { $null = Click-Point $hit2.center } else { $okOptions = $false }
          } else { $okOptions = $false }
        }

        if ($okOptions) {
          $submitTexts = if ($name -eq 'question-multi-turn') { @('检查答案', '确认提交') } else { @('提交回答', '检查答案') }
          $submit = $null
          $deadline = (Get-Date).AddSeconds(15)
          while ((Get-Date) -lt $deadline -and -not $submit) {
            $region = Get-IslandRect
            foreach ($t in $submitTexts) {
              $cand = Find-AnyText -Region $region -Texts @($t) -SaveImage "$shotBase-05-submit.png" -PreferBottom
              if ($cand) { $submit = $cand; break }
            }
            if (-not $submit) { Start-Sleep -Milliseconds 500 }
          }
          $verdict.steps += [pscustomobject]@{ step = '找到提交按钮'; ok = [bool]$submit; text = $(if ($submit) { $submit.text } else { $null }) }
          if (-not $submit) { $verdict.error = '没有识别到提交类按钮' }
          else {
            $null = Click-Point $submit.center
            if ($submit.text -eq '检查答案') {
              $region = Get-IslandRect
              $confirm = Wait-Text -Region $region -Texts @('确认提交') -TimeoutSeconds 10 -ShotOnFound "$shotBase-06-review.png" -ShotOnTimeout "$shotBase-06-timeout.png"
              $verdict.steps += [pscustomobject]@{ step = '审阅页确认提交'; ok = [bool]$confirm; text = $(if ($confirm) { $confirm.text } else { $null }) }
              if ($confirm) { $null = Click-Point $confirm.center }
              else { $verdict.error = '审阅页没有识别到「确认提交」' }
            }
          }
        } else {
          $verdict.error = '没有点到期望的选项'
        }
      }
    }
    elseif ($name -eq 'done') {
      $hit = Wait-Text -Region $region -Texts @('本轮已结束', '本轮已', '查看回复') -TimeoutSeconds 25 -ShotOnFound "$shotBase-01-done.png" -ShotOnTimeout "$shotBase-01-timeout.png"
      $verdict.steps += [pscustomobject]@{ step = '等到完成通知'; ok = [bool]$hit; text = $(if ($hit) { $hit.text } else { $null }) }
      if (-not $hit) { $verdict.error = '25 秒内没有在岛上看到完成通知' }
      else {
        # 先抓「查看回复」：完成卡停留时间有限（dwell=15s），先点开结果层再慢慢验正文。
        # 这一步不是判定项（主 agent 明确：完成通知本身无需点击即可判 done），所以失败只记录不判错。
        $region = Get-IslandRect
        $open = Find-AnyText -Region $region -Texts @('查看回复', '查看') -SaveImage "$shotBase-02-review.png" -Scales @(2, 3, 4, 5, 6, 8)
        $verdict.steps += [pscustomobject]@{ step = '打开「查看回复」'; ok = [bool]$open; text = $(if ($open) { $open.text } else { $null }); fatal = $false }
        $marker = $null
        if ($open) {
          $null = Click-Point $open.center
          Start-Sleep -Milliseconds 900
          $region = Get-IslandRect
          $result = Wait-Text -Region $region -Texts @('本轮回复', '复制内容', '可滚动阅读') -TimeoutSeconds 10 -ShotOnFound "$shotBase-03-result.png" -ShotOnTimeout "$shotBase-03-timeout.png"
          $verdict.steps += [pscustomobject]@{ step = '看到结果正文层'; ok = [bool]$result; text = $(if ($result) { $result.text } else { $null }); fatal = $false }
          if ($result) {
            # 结果层是「本轮回复」全文，在这里验正文标记最稳（不会过期）。
            $region = Get-IslandRect
            $marker = Wait-Text -Region $region -Texts @('daozai-e2e-done', 'e2e-done') -TimeoutSeconds 6 -ShotOnFound "$shotBase-03b-body.png" -ShotOnTimeout "$shotBase-03b-body-timeout.png"
          }
        }
        if (-not $marker) {
          # 兜底：完成卡本身也带正文摘要（completion=true / content=2）。
          $region = Get-IslandRect
          $marker = Wait-Text -Region $region -Texts @('daozai-e2e-done', 'e2e-done') -TimeoutSeconds 8 -ShotOnFound "$shotBase-01b-body.png" -ShotOnTimeout "$shotBase-01b-body-timeout.png"
        }
        $verdict.steps += [pscustomobject]@{ step = '完成正文含 e2e 标记'; ok = [bool]$marker; text = $(if ($marker) { $marker.line } else { $null }) }
        if (-not $marker) { $verdict.error = '完成卡/结果层都没有识别到回复正文（检查「任务完成提醒」和「完成内容=本轮回复」）' }
      }
    }

    # 等驱动收敛并拿结论
    if ($verdict.error) {
      # 交互失败就别等满 --wait：给驱动 3 秒收尾（例如详情已点到但截图晚了一步），否则杀掉。
      $exited = Wait-DriverExit -DriverPid $driver.Id -TimeoutSeconds 3
      if (-not $exited) { Stop-Process -Id $driver.Id -Force -ErrorAction SilentlyContinue }
    } else {
      $exited = Wait-DriverExit -DriverPid $driver.Id -TimeoutSeconds ($WaitSeconds + 5)
      if (-not $exited) {
        Stop-Process -Id $driver.Id -Force -ErrorAction SilentlyContinue
        $verdict.error = "驱动在 $($WaitSeconds + 5) 秒内没有退出（卡片一直没作答？）"
      }
    }
    # 驱动进程退出与重定向文件落盘之间有毫秒级窗口，别只读一次：
    # 第一轮实测就踩到「进程已退、文件还没刷出来」，把成功的场景判成失败。
    $json = $null
    $readDeadline = (Get-Date).AddSeconds(5)
    while ((Get-Date) -lt $readDeadline -and -not $json) {
      $json = Read-DriverJson $stdout
      if (-not $json) { Start-Sleep -Milliseconds 200 }
    }
    $verdict.driver = $json
    if ($NoClick) {
      # 视觉检查模式：驱动被主动结束，不判桥接决定。
    }
    elseif (-not $json) {
      if (-not $verdict.error) { $verdict.error = '驱动没有输出 JSON（见 *-driver.err.log）' }
    }
    else {
      # 屏幕侧点击 与 桥接侧决定 必须相互印证。
      if ($name -eq 'approval') {
        $bridge = @($json.bridge)
        $entry = if ($bridge.Count -ge 1) { $bridge[0] } else { $null }
        $reply = if ($entry) { $entry.reply } else { $null }
        # 宿主只有在「岛仔返回 allow 且操作内容匹配」时才给 allowed-once，所以它本身就是强证据。
        if ($json.ok -and $reply -and $reply.decision -eq 'allow' -and $json.decision -eq 'allowed-once' -and -not $json.handback) { $verdict.bridgeOk = $true }
        elseif (-not $verdict.error) { $verdict.error = "桥接侧没有收到 allow（decision=$($json.decision), handback=$($json.handback)）" }
      }
      elseif ($name -like 'question*') {
        $answers = @()
        if ($json.decision -and $json.decision.answers) { $answers = @($json.decision.answers) }
        if ($json.ok -and $answers.Count -ge 1) {
          $selected = @($answers[0].selected)
          $expectedFirst = $spec.options[0]
          if ($selected -contains $expectedFirst) { $verdict.bridgeOk = $true }
          elseif (-not $verdict.error) { $verdict.error = "桥接侧收到 $($selected -join ',')，与屏幕上点的「$expectedFirst」不一致" }
        }
        elseif (-not $verdict.error) { $verdict.error = "桥接侧没有收到重答答案（decision=$($json.decision)）" }
      }
      elseif ($name -eq 'done') {
        $finished = @(@($json.bridge) | Where-Object { $_.kind -eq 'finished' })
        $entry = if ($finished.Count -ge 1) { $finished[0] } else { $null }
        $textLength = if ($entry -and $entry.sent -and $entry.sent.text) { $entry.sent.text.Length } else { 0 }
        # content=2（本轮回复）时 makePayload 的 mode 必须是 2，且正文非空：证明全文真的发给了岛仔。
        if ($json.ok -and $json.decision -eq 'done' -and $entry -and $entry.reply.done -and $entry.mode -eq 2 -and $textLength -gt 0) { $verdict.bridgeOk = $true }
        elseif (-not $verdict.error) { $verdict.error = "桥接侧没有确认完成事件（decision=$($json.decision), mode=$($entry.mode), textLen=$textLength）" }
      }
    }

    # 场景通过 = 「屏幕上点的」与「桥接侧收到的」都对上，且没有阻断性错误。
    if (-not $NoClick) { $verdict.ok = ($null -eq $verdict.error -and $verdict.bridgeOk) }
    $verdict.duration = [Math]::Round(((Get-Date) - $scenarioStart).TotalSeconds, 1)
    $script:Results += $verdict
    if ($verdict.ok) { Write-Step "场景 $name 通过（$($verdict.duration)s）" }
    else { Write-Step "场景 $name 失败：$($verdict.error)"; $script:Failures += "${name}: $($verdict.error)" }

    # 场景之间留点时间，让上一张卡片退场。
    Start-Sleep -Milliseconds 800
  }
} finally {
  if (-not $KeepAlive) { Stop-Island }
  Restore-Env
}

# ---------------------------------------------------------------- 汇总

$summary = [pscustomobject]@{
  ok        = ($script:Failures.Count -eq 0)
  at        = (Get-Date).ToString('s')
  exe       = $Exe
  runCopy   = $runExe
  dataDir   = $DataDir
  shots     = $script:Shots
  scenarios = $script:Results
  failures  = $script:Failures
  log       = $script:Log
}
Save-Summary $summary
Write-Host ''
Write-Host "截图与结果：$($script:Shots)"
Write-Host ($summary | ConvertTo-Json -Depth 8)
if ($script:Failures.Count) { exit 3 }
exit 0
