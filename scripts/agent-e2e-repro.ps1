<#
岛仔「点击无反应」真机复现 / 回归脚本（阶段1 复现，阶段2 回归共用）。

由 scripts\agent-e2e.ps1 改写（原脚本保持不动），差异：
  * 用用户真实配置 agent_cards.content=0（仅状态）、expanded=true、deferred=false、dwell 可配（默认 15）、island_size=140；
  * 不判「卡片能不能弹出来」，专测两处点击之后有没有可见结果：
      - done-repro ：完成卡点「查看回复」→ 记录点击前后截图与 OCR，判断结果层是否打开；
      - jump-repro ：提问详情卡头部点「切换到客户端」→ 记录点击前后 GetForegroundWindow、截图与 OCR，
                     判断是否出现 toast / 前台是否变化；随后把 2 轮问题答完，验证桥接仍拿到答案；
  * 每个检查点都留「整块截图 + 卡片裁剪图 + OCR JSON」；桥接结论仍取 agent-drive.cjs 的输出；
  * -ViewReplyPoint / -ChipPoint 可在 OCR 认不出小字号 chip 时手工指定点击坐标；
  * -DshFront / -DshBack：点击前把 DSH 置前 / 确保 DSH 不在前台（lead 要求的两种真实场景）。

用法（仓库根目录，PowerShell 5.1）：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e-repro.ps1 -Scenario done-repro
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e-repro.ps1 -Scenario jump-repro -DshFront
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e-repro.ps1 -Scenario jump-repro -DshBack

退出码：0 证据采集完整（bug 复现或修复通过都算 0）；2 前置条件不满足；3 关键步骤失败；4 已有岛仔在运行。

注意：本文件必须以 UTF-8 with BOM 保存，否则 Windows PowerShell 5.1 会按本地代码页解析中文而报错。
#>
[CmdletBinding()]
param(
  [string]$Exe,
  [ValidateSet('done-repro', 'jump-repro', 'scroll-repro', 'banner-jump-repro')]
  [string]$Scenario = 'done-repro',
  [string]$DataDir,
  [string]$OutRoot,
  [int]$WaitSeconds = 90,
  [int]$Dwell = 15,
  # 完成内容（0=仅状态 / 1=回复摘要 / 2=本轮回复）。scroll-repro 需要 2 才有正文层可滚。
  [int]$AgentContent = 0,
  [string]$ViewReplyPoint,
  [string]$ChipPoint,
  [switch]$DshFront,
  [switch]$DshBack,
  [switch]$KeepAlive,
  [switch]$NoClick
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Threading;
public static class DaozaiReproPower {
  [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint f);
  public static void On() { SetThreadExecutionState(0x80000000u | 0x1u | 0x2u); }
  public static void Off() { SetThreadExecutionState(0x80000000u); }
}
public static class DaozaiRepro {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr param);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr param);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  // GetCurrentThreadId 在 kernel32，不在 user32（写错会 EntryPointNotFoundException）。
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool attach);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, StringBuilder text, int max);

  public static string Title(IntPtr hwnd) {
    StringBuilder sb = new StringBuilder(512);
    GetWindowTextW(hwnd, sb, 512);
    return sb.ToString();
  }
  // 按 PID 找岛仔主窗口：比按标题精确匹配稳（标题含中文，跨进程代码页容易踩坑）。
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
  // 在一组 PID 里挑面积最大的可见窗口（DSH 桌面端主窗口）。
  public static IntPtr FindLargestWindowAmong(uint[] pids) {
    IntPtr best = IntPtr.Zero;
    long bestArea = 0;
    EnumWindows(delegate(IntPtr hwnd, IntPtr param) {
      uint owner;
      GetWindowThreadProcessId(hwnd, out owner);
      bool match = false;
      for (int i = 0; i < pids.Length; i++) { if (pids[i] == owner) { match = true; break; } }
      if (!match || !IsWindowVisible(hwnd)) return true;
      RECT rect;
      if (!GetWindowRect(hwnd, out rect)) return true;
      long w = rect.Right - rect.Left;
      long h = rect.Bottom - rect.Top;
      if (w < 400 || h < 300) return true;
      long area = w * h;
      if (area > bestArea) { bestArea = area; best = hwnd; }
      return true;
    }, IntPtr.Zero);
    return best;
  }
  // AttachThreadInput + SetForegroundWindow：绕过前台锁，等价用户点任务栏切窗口。
  public static bool ForceForeground(IntPtr hwnd) {
    if (hwnd == IntPtr.Zero) return false;
    if (IsIconic(hwnd)) ShowWindow(hwnd, 9);
    IntPtr fg = GetForegroundWindow();
    uint fgPid;
    uint fgThread = GetWindowThreadProcessId(fg, out fgPid);
    uint me = GetCurrentThreadId();
    bool attached = false;
    if (fgThread != 0 && fgThread != me) attached = AttachThreadInput(me, fgThread, true);
    bool ok = SetForegroundWindow(hwnd);
    if (!ok) { BringWindowToTop(hwnd); ok = SetForegroundWindow(hwnd); }
    if (attached) AttachThreadInput(me, fgThread, false);
    return ok;
  }
  public static void Nudge() {
    SetCursorPos(4, 4);
    mouse_event(0x0001, 2, 0, 0, UIntPtr.Zero);
    mouse_event(0x0001, unchecked((uint)-2), 0, 0, UIntPtr.Zero);
  }
  // dwData 是带符号的滚动量：向下滚是负数（-120 = 一格）。
  public static void Wheel(int x, int y, int delta) {
    SetCursorPos(x, y);
    Thread.Sleep(40);
    mouse_event(0x0800, 0, 0, unchecked((uint)delta), UIntPtr.Zero);
    Thread.Sleep(30);
  }
  public static bool Click(int x, int y) {
    if (!SetCursorPos(x, y)) return false;
    Thread.Sleep(60);
    mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
    Thread.Sleep(40);
    mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    Thread.Sleep(80);
    return true;
  }
}
'@

[DaozaiReproPower]::On()

$script:RepoRoot = Split-Path -Parent $PSScriptRoot
$script:Shots = $null
$script:Log = @()
$script:Steps = @()
$script:ProcessLog = @()
$script:Results = @()
$script:Failures = @()
$script:LastRegion = $null
$script:islandPid = $null
$script:UiScale = 1.4
$script:CardWidthPx = 419 * $script:UiScale
$script:TracePath = Join-Path $env:TEMP 'daozai-agent-trace.log'

function Write-Step([string]$message) {
  $line = '[{0}] {1}' -f (Get-Date -Format 'HH:mm:ss'), $message
  $script:Log += $line
  Write-Host $line
}
function Save-Summary($payload) {
  if ($script:Shots) {
    $path = Join-Path $script:Shots 'summary.json'
    [System.IO.File]::WriteAllText($path, ($payload | ConvertTo-Json -Depth 12), (New-Object System.Text.UTF8Encoding($false)))
  }
}
function Fail([int]$code, [string]$message) {
  Write-Host "失败：$message" -ForegroundColor Red
  Save-Summary ([pscustomobject]@{ ok = $false; code = $code; error = $message; log = $script:Log; results = $script:Results })
  exit $code
}
function Note-Process([string]$what, $procId, [string]$path) {
  $item = [pscustomobject]@{ what = $what; pid = $procId; path = $path; at = (Get-Date).ToString('s') }
  $script:ProcessLog += $item
  Write-Step ('进程：' + ($item | ConvertTo-Json -Compress))
}

# ---------------------------------------------------------------- 屏幕 / 窗口

function Get-ScreenBounds {
  $v = [System.Windows.Forms.SystemInformation]::VirtualScreen
  [pscustomobject]@{ left = $v.Left; top = $v.Top; right = $v.Right; bottom = $v.Bottom }
}

# 岛仔窗口本身就是卡片画布（CANVAS_W 逻辑宽，卡片在中间）；直接拿窗口矩形当识别区域，
# 比原脚本按「虚拟屏幕中心」推算稳（本机双屏，虚拟屏中心落在两块屏的接缝上）。
function Get-IslandRegion {
  if (-not $script:islandPid) { return $null }
  $hwnd = [DaozaiRepro]::FindWindowForProcess([uint32]$script:islandPid)
  if ($hwnd -eq [IntPtr]::Zero) { return $null }
  $rect = New-Object DaozaiRepro+RECT
  if (-not [DaozaiRepro]::GetWindowRect($hwnd, [ref]$rect)) { return $null }
  if (($rect.Right - $rect.Left) -le 0 -or ($rect.Bottom - $rect.Top) -le 0) { return $null }
  $screen = Get-ScreenBounds
  [pscustomobject]@{
    left   = [Math]::Max($screen.left, $rect.Left - 12)
    right  = [Math]::Min($screen.right, $rect.Right + 12)
    top    = [Math]::Max($screen.top, $rect.Top - 6)
    bottom = [Math]::Min([Math]::Min($screen.bottom, 800), $rect.Bottom + 60)
    hwnd   = $hwnd.ToInt64()
    winLeft = $rect.Left; winTop = $rect.Top; winRight = $rect.Right; winBottom = $rect.Bottom
  }
}
function Wait-IslandRegion([int]$TimeoutSeconds = 15) {
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    $r = Get-IslandRegion
    if ($r) { return $r }
    Start-Sleep -Milliseconds 200
  }
  return $null
}
function Get-LiveRegion($fallback) {
  if ($script:islandPid) {
    $fresh = Get-IslandRegion
    if ($fresh) { $script:LastRegion = $fresh; return $fresh }
  }
  if ($script:LastRegion) { return $script:LastRegion }
  if ($fallback) { return $fallback }
  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $cx = [int]($b.Left + $b.Width / 2)
  $half = [int][Math]::Ceiling($script:CardWidthPx / 2) + 80
  [pscustomobject]@{ left = $cx - $half; right = $cx + $half; top = $b.Top; bottom = [Math]::Min(800, $b.Bottom) }
}
function Get-RegionString($region) { "$($region.left),$($region.top),$($region.right),$($region.bottom)" }

# ---------------------------------------------------------------- OCR

function Invoke-OcrDump {
  param($Region, [int]$Scale = 3, [string]$Shot, [string]$JsonOut)
  $Region = Get-LiveRegion $Region
  $cli = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $script:ocrScript, '-Region', (Get-RegionString $Region), '-Scale', [string]$Scale)
  if ($Shot) { $cli += @('-SaveImage', $Shot) }
  if ($JsonOut) { $cli += @('-OutFile', $JsonOut) }
  $previous = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try { $raw = & powershell @cli 2>&1 | Out-String }
  finally { $ErrorActionPreference = $previous }
  try { return ($raw | ConvertFrom-Json) } catch {
    return [pscustomobject]@{ ok = $false; lines = @(); error = ('OCR 输出无法解析: ' + $raw.Substring(0, [Math]::Min(200, $raw.Length))) }
  }
}

function Find-LineMatch {
  # PreferTop：同一候选词命中多处时取最靠上的（头部 chip 与底部「回客户端」按钮都含「客户端」）。
  # MaxExtra >= 0：只接受「行内字数 ≤ 目标词字数 + MaxExtra」的行，用来把选项行和题干区分开
  #（题干也会写「请选择「方案一」」，选项行只多一个序号徽标「1方案一」）。
  param($Ocr, [string[]]$Texts, [switch]$PreferBottom, [switch]$PreferTop, [int]$MaxExtra = -1)
  if (-not $Ocr -or -not $Ocr.ok) { return $null }
  $lines = @($Ocr.lines)
  foreach ($text in $Texts) {
    $needle = ($text -replace '\s+', '')
    if (-not $needle) { continue }
    $hits = @()
    foreach ($line in $lines) {
      $haystack = [string]$line.compact
      if ($MaxExtra -ge 0 -and $haystack.Length -gt ($needle.Length + $MaxExtra)) { continue }
      $at = $haystack.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase)
      while ($at -ge 0) {
        $ratio = $at / [Math]::Max(1, $haystack.Length)
        $lenRatio = $needle.Length / [Math]::Max(1, $haystack.Length)
        $mx = $line.rect[0] + [int](($ratio + $lenRatio / 2) * $line.rect[2])
        $my = $line.center[1]
        $hits += [pscustomobject]@{ center = @($mx, $my); line = $line.text; rect = $line.rect }
        $at = $haystack.IndexOf($needle, $at + $needle.Length, [StringComparison]::OrdinalIgnoreCase)
      }
    }
    if ($hits.Count) {
      $pick = $hits[0]
      if ($PreferTop -and $hits.Count -gt 1) {
        $pick = $hits | Sort-Object -Property @{ Expression = { $_.center[1] } } | Select-Object -First 1
      }
      elseif ($PreferBottom -and $hits.Count -gt 1) {
        $pick = $hits | Sort-Object -Property @{ Expression = { $_.center[1] } } | Select-Object -Last 1
      }
      return [pscustomobject]@{ found = $true; center = $pick.center; text = $text; line = $pick.line; matches = $hits }
    }
  }
  return $null
}
function Find-InDump { param($Dump, [string[]]$Texts, [switch]$PreferBottom, [switch]$PreferTop, [int]$MaxExtra = -1) Find-LineMatch -Ocr $Dump -Texts $Texts -PreferBottom:$PreferBottom -PreferTop:$PreferTop -MaxExtra $MaxExtra }

# 多倍率找词：每倍率一次 OCR 子进程（命中即返回）；只在第一次带 -Shot 存图。
function Find-TextSmart {
  param($Region, [string[]]$Texts, [int[]]$Scales = @(3, 2, 4, 6), [string]$Shot, [string]$JsonOut, [switch]$PreferBottom, [switch]$PreferTop, [int]$MaxExtra = -1)
  $first = $true
  foreach ($scale in $Scales) {
    $shot = if ($first) { $Shot } else { $null }
    $json = if ($first) { $JsonOut } else { $null }
    $dump = Invoke-OcrDump -Region $Region -Scale $scale -Shot $shot -JsonOut $json
    if ($dump -and $dump.ok) {
      $m = Find-LineMatch -Ocr $dump -Texts $Texts -PreferBottom:$PreferBottom -PreferTop:$PreferTop -MaxExtra $MaxExtra
      if ($m) { return [pscustomobject]@{ found = $true; center = $m.center; text = $m.text; line = $m.line; scale = $scale; matches = $m.matches } }
    }
    $first = $false
  }
  return $null
}

# 轮询直到任意候选文字出现（默认 3/2 倍率，命中即返回）。
function Wait-Text {
  param($Region, [string[]]$Texts, [int]$TimeoutSeconds = 30, [int[]]$Scales = @(3, 2))
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $attempt = 0
  while ((Get-Date) -lt $deadline) {
    $attempt++
    [DaozaiRepro]::Nudge()
    foreach ($scale in $Scales) {
      $dump = Invoke-OcrDump -Region $Region -Scale $scale
      $m = Find-InDump $dump $Texts
      if ($m) { return [pscustomobject]@{ found = $true; center = $m.center; text = $m.text; line = $m.line; attempts = $attempt } }
    }
    Start-Sleep -Milliseconds 250
  }
  return $null
}

# 找「岛仔卡片」在图片里的包围盒（图片像素坐标）。
# 判据不能用「整行深色占比」：卡片头部有浅色 chip、底部按钮是浅色填充，占比会被压到阈值以下，
# 于是把标题行/按钮行当成卡片结束（实测踩过：得到 24px 高的假盒子）。改成「一行里存在 >=240px
# 的连续近黑」就算卡片行。
function Get-DarkBoxInPng {
  param([string]$Path)
  $bmp = New-Object System.Drawing.Bitmap($Path)
  try {
    $w = $bmp.Width; $h = $bmp.Height; $limit = 150
    $rowOk = New-Object 'bool[]' $h
    for ($y = 0; $y -lt $h; $y += 2) {
      $run = 0; $best = 0
      for ($x = 0; $x -lt $w; $x += 4) {
        $c = $bmp.GetPixel($x, $y)
        if (($c.R + $c.G + $c.B) -lt $limit) { $run += 4; if ($run -gt $best) { $best = $run } } else { $run = 0 }
      }
      $rowOk[$y] = ($best -ge 240)
    }
    $rowStart = -1; $rowEnd = -1; $gap = 0; $seen = $false
    for ($y = 0; $y -lt $h; $y += 2) {
      if ($rowOk[$y]) { if ($rowStart -lt 0) { $rowStart = $y }; $rowEnd = $y; $gap = 0; $seen = $true }
      elseif ($seen) { $gap += 2; if ($gap -gt 30) { break } }
      elseif ($y -gt 40) { break }
    }
    if ($rowStart -lt 0) { return $null }
    $colStart = -1; $colEnd = -1
    for ($x = 0; $x -lt $w; $x += 2) {
      $cnt = 0; $tot = 0
      for ($y = $rowStart; $y -le $rowEnd; $y += 2) { $c = $bmp.GetPixel($x, $y); $tot++; if (($c.R + $c.G + $c.B) -lt $limit) { $cnt++ } }
      if (($cnt / [Math]::Max(1, $tot)) -ge 0.5) { if ($colStart -lt 0) { $colStart = $x }; $colEnd = $x }
      elseif ($colStart -ge 0 -and ($x - $colEnd) -gt 40) { break }
    }
    if ($colStart -lt 0 -or ($colEnd - $colStart) -lt 80) { return $null }
    return [pscustomobject]@{ left = $colStart; top = $rowStart; right = $colEnd; bottom = $rowEnd }
  } finally { $bmp.Dispose() }
}

# 把卡片从整块截图里裁出来（卡片不一定在窗口画布正中：本机窗口 1344px、卡片 587px、卡片中心=主屏中心）。
function Crop-CardPng {
  param([string]$In, [string]$Out, $Region)
  try {
    $box = Get-DarkBoxInPng -Path $In
    if (-not $box) { Copy-Item -LiteralPath $In -Destination $Out -Force; return }
    $bmp = New-Object System.Drawing.Bitmap($In)
    try {
      $pad = 12
      $x0 = [Math]::Max(0, $box.left - $pad); $y0 = [Math]::Max(0, $box.top - $pad)
      $cw = [Math]::Min($bmp.Width - $x0, ($box.right - $box.left + 1) + $pad * 2)
      $ch = [Math]::Min($bmp.Height - $y0, ($box.bottom - $box.top + 1) + $pad * 2)
      $crop = New-Object System.Drawing.Bitmap($cw, $ch)
      $g = [System.Drawing.Graphics]::FromImage($crop)
      try { $g.DrawImage($bmp, (New-Object System.Drawing.Rectangle(0, 0, $cw, $ch)), (New-Object System.Drawing.Rectangle($x0, $y0, $cw, $ch)), [System.Drawing.GraphicsUnit]::Pixel) } finally { $g.Dispose() }
      $crop.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
      $crop.Dispose()
    } finally { $bmp.Dispose() }
  } catch { Write-Step "裁卡片图失败：$($_.Exception.Message)"; try { Copy-Item -LiteralPath $In -Destination $Out -Force } catch { } }
}

# 卡片在屏幕上的暗色包围盒（按钮禁用时字是暗灰、OCR 读不到，就用几何位置兜底点击）。
function Get-CardBox {
  param($Region)
  $region = Get-LiveRegion $Region
  $tmp = Join-Path $env:TEMP ('daozai-box-' + [guid]::NewGuid().ToString('N') + '.png')
  $cli = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $script:ocrScript, '-Region', (Get-RegionString $region), '-Scale', '1', '-SaveImage', $tmp)
  $previous = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try { $null = & powershell @cli 2>&1 | Out-String } finally { $ErrorActionPreference = $previous }
  if (-not (Test-Path $tmp)) { return $null }
  $box = $null
  try {
    $b = Get-DarkBoxInPng -Path $tmp
    if ($b) {
      $box = [pscustomobject]@{ left = $region.left + $b.left; top = $region.top + $b.top; right = $region.left + $b.right; bottom = $region.top + $b.bottom }
    }
  } catch { $box = $null } finally { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
  return $box
}

# 卡片底部按钮行右边那颗按钮（下一题 / 检查答案 / 确认提交）的几何中心。
function Get-FooterRightPoint {
  param($Region)
  $box = Get-CardBox $Region
  if (-not $box) { return $null }
  $pad = 14 * $script:UiScale
  $btnH = 28 * $script:UiScale
  $gap = 8 * $script:UiScale
  $btnW = (($box.right - $box.left) - $pad * 2 - $gap) / 2
  if ($btnW -lt 40) { return $null }
  return @([int]($box.right - $pad - $btnW / 2), [int]($box.bottom - $pad - $btnH / 2 + 3))
}

# 一个检查点 = 整块截图 + 卡片裁剪图 + OCR JSON（返回 OCR 结果供判定）。
function Save-Capture {
  param($Region, [string]$Base)
  $region = Get-LiveRegion $Region
  $raw = "$Base-raw.png"
  $dump = Invoke-OcrDump -Region $region -Scale 3 -Shot $raw -JsonOut "$Base-ocr.json"
  Crop-CardPng -In $raw -Out "$Base-card.png" -Region $region
  return $dump
}

function Click-Point($center) {
  $x = [int]$center[0]; $y = [int]$center[1]
  if ($NoClick) { Write-Step "  （-NoClick）本应点击 ($x,$y)"; return $true }
  $ok = [DaozaiRepro]::Click($x, $y)
  Write-Step "  点击 ($x,$y) -> $ok"
  Start-Sleep -Milliseconds 500
  return $ok
}
function Parse-Point([string]$text) {
  if (-not $text) { return $null }
  $parts = $text -split '[,;\s]+' | Where-Object { $_ -ne '' }
  if ($parts.Count -ne 2) { return $null }
  return @([int]$parts[0], [int]$parts[1])
}

# ---------------------------------------------------------------- 前台窗口

function Get-WindowMeta([IntPtr]$hwnd) {
  if ($hwnd -eq [IntPtr]::Zero) { return [pscustomobject]@{ hwnd = 0; pid = 0; process = ''; title = '' } }
  $ownerPid = 0
  [void][DaozaiRepro]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid)
  $proc = ''
  try { $proc = (Get-Process -Id $ownerPid -ErrorAction Stop).ProcessName } catch { $proc = '?' }
  [pscustomobject]@{ hwnd = $hwnd.ToInt64(); pid = $ownerPid; process = $proc; title = [DaozaiRepro]::Title($hwnd) }
}
function Get-ForegroundMeta { return Get-WindowMeta ([DaozaiRepro]::GetForegroundWindow()) }

function Find-WindowByProcessNames([string[]]$names) {
  $ids = @()
  foreach ($n in $names) {
    $ids += @(Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
  }
  if (-not $ids.Count) { return [IntPtr]::Zero }
  return [DaozaiRepro]::FindLargestWindowAmong([uint32[]]$ids)
}
function Set-DshForeground {
  $hwnd = Find-WindowByProcessNames @('DeepSeek Harness')
  if ($hwnd -eq [IntPtr]::Zero) { return $false }
  return [DaozaiRepro]::ForceForeground($hwnd)
}
# DSH 在前台时换成别的窗口（不点击，只切前台），模拟「用户在看浏览器」。
function Ensure-DshNotForeground {
  $fg = Get-ForegroundMeta
  if ($fg.process -eq 'DeepSeek Harness') {
    $hwnd = Find-WindowByProcessNames @('msedge', 'chrome', 'explorer')
    if ($hwnd -ne [IntPtr]::Zero) { [void][DaozaiRepro]::ForceForeground($hwnd); Start-Sleep -Milliseconds 600 }
  }
  return (Get-ForegroundMeta)
}

# ---------------------------------------------------------------- trace

function Save-Trace([string]$Base) {
  if (-not (Test-Path $script:TracePath)) { Write-Step "trace 日志不存在：$($script:TracePath)"; return $null }
  try {
    $all = [System.IO.File]::ReadAllLines($script:TracePath)
    $tail = if ($all.Count -gt 300) { $all[($all.Count - 300)..($all.Count - 1)] } else { $all }
    $out = "$Base-trace-tail.log"
    [System.IO.File]::WriteAllLines($out, $tail, (New-Object System.Text.UTF8Encoding($false)))
    Write-Step "trace 日志尾部 $($tail.Count)/$($all.Count) 行 -> $out"
    return $out
  } catch { Write-Step "读 trace 失败：$($_.Exception.Message)"; return $null }
}

# ---------------------------------------------------------------- 进程

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
  Set-EnvVar 'DAOZAI_AGENT_TRACE' '1'
  $p = Start-Process -FilePath $script:runExe -PassThru
  $script:islandPid = $p.Id
  Note-Process '启动岛仔（被测实例）' $script:islandPid $script:runExe
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
    if ($p) {
      Stop-Process -Id $script:islandPid -Force -ErrorAction SilentlyContinue
      Note-Process '停止岛仔（被测实例）' $script:islandPid $script:runExe
    }
    $script:islandPid = $null
  }
  Start-Sleep -Milliseconds 600
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
    $text = [System.IO.File]::ReadAllText($StdoutFile, (New-Object System.Text.UTF8Encoding($false)))
  } catch { return $null }
  if (-not $text) { return $null }
  $line = [regex]::Split($text.Trim(), '\r?\n') | Where-Object { $_.TrimStart().StartsWith('{') } | Select-Object -Last 1
  if (-not $line) { return $null }
  try { return ($line | ConvertFrom-Json) } catch { return $null }
}

# ---------------------------------------------------------------- 前置检查

$dshExe = Join-Path $env:LOCALAPPDATA 'Programs\DeepSeek Harness\DeepSeek Harness.exe'
if (-not (Test-Path $dshExe)) { Fail 2 "找不到 DeepSeek Harness：$dshExe" }
Write-Step "DSH 宿主：$dshExe"

if (-not $Exe) { $Exe = Join-Path $script:RepoRoot 'target\x86_64-pc-windows-msvc\release\daozai.exe' }
if (-not (Test-Path $Exe)) { Fail 2 "找不到岛仔 exe：$Exe" }
$Exe = (Resolve-Path -LiteralPath $Exe).Path
Write-Step "岛仔 exe：$Exe（$((Get-Item $Exe).LastWriteTime)，SHA256 $((Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.Substring(0,16))）"

# 只认真正的岛仔进程名（daozai / daozai-e2e）：cargo 构建脚本进程名形如 daozai-<hash>，不能误判成本实例。
$other = @(Get-Process -Name 'daozai', 'daozai-e2e' -ErrorAction SilentlyContinue)
if ($other.Count) {
  Fail 4 ("已有岛仔在运行（PID $(($other | ForEach-Object { $_.Id }) -join ', ')），单实例互斥体会互踢。" + [Environment]::NewLine + '请先停止现有实例再跑本脚本（脚本不会动别人的岛仔）。')
}

$script:ocrScript = Join-Path $PSScriptRoot 'agent-screen-ocr.ps1'
$driveScript = Join-Path $script:RepoRoot 'tests\fixtures\agent-drive.cjs'
foreach ($f in @($script:ocrScript, $driveScript)) { if (-not (Test-Path $f)) { Fail 2 "缺少文件：$f" } }

$ocrProbeRaw = & powershell -NoProfile -ExecutionPolicy Bypass -File $script:ocrScript -Region '0,0,64,64' -Scale 1 2>&1 | Out-String
try { $ocrProbe = $ocrProbeRaw | ConvertFrom-Json } catch { Fail 2 "OCR 自检输出无法解析：$($ocrProbeRaw.Substring(0, [Math]::Min(300, $ocrProbeRaw.Length)))" }
if (-not $ocrProbe.ok) { Fail 2 "OCR 自检失败：$($ocrProbe.error)" }
Write-Step "OCR 可用：语言 $($ocrProbe.language)"

# ---------------------------------------------------------------- 数据目录 / 设置

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if (-not $DataDir) { $DataDir = Join-Path $env:TEMP "daozai-repro-$stamp-$Scenario" }
$DataDir = [System.IO.Path]::GetFullPath($DataDir)
if (-not $OutRoot) { $OutRoot = Join-Path $script:RepoRoot 'docs\superpowers\research\agent-e2e-shots' }
$script:Shots = Join-Path $OutRoot "$stamp-$Scenario"
New-Item -ItemType Directory -Force -Path $DataDir, $script:Shots, (Join-Path $DataDir 'dsh-home\profiles\desktop'), (Join-Path $DataDir 'codex-home') | Out-Null
Write-Step "数据目录：$DataDir"
Write-Step "截图目录：$($script:Shots)"
[System.IO.File]::WriteAllText((Join-Path $DataDir 'dsh-home\profiles\desktop\package.json'), '{"private":true}', (New-Object System.Text.UTF8Encoding($false)))

$script:runExe = Join-Path $DataDir 'daozai-e2e.exe'
Copy-Item -LiteralPath $Exe -Destination $script:runExe -Force
Write-Step "运行副本：$($script:runExe)"

# 用户真实配置：完成内容=仅状态(content=0)、展开卡、不延后、dwell 可配；island_size=140 给 OCR 用。
$agentPrefs = ConvertTo-Json -Compress -InputObject ([ordered]@{ interactive = $true; handoff = 0; completion = $true; dwell = $Dwell; expanded = $true; content = $AgentContent; deferred = $false; system_toast = $false })
$settings = @(
  'notifications=1', 'system_notify=0', 'sound=0', 'read_notifications=0',
  'dsh_notifications=1', 'codex_notifications=0',
  "agent_cards=$agentPrefs",
  'hide_fullscreen=0', 'avoid_top_bar=0', 'pet=0', 'island_size=140'
)
[System.IO.File]::WriteAllLines((Join-Path $DataDir 'settings.txt'), $settings, (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText((Join-Path $script:Shots 'settings.txt'), (($settings -join [Environment]::NewLine) + [Environment]::NewLine), (New-Object System.Text.UTF8Encoding($false)))
Write-Step "设置：agent_cards=$agentPrefs"

# ---------------------------------------------------------------- 场景参数

$doneText = @(
  'E2E 完成正文：岛仔端到端验收。',
  '- 标记 daozai-e2e-done',
  '- 如果你在岛上看到这段话，说明真实 DSH 宿主 -> 桥接 -> 岛仔的完成链路是通的。'
) -join ([char]10)

# 长回复：80 行占位正文 + 末行标记，用来验证结果层能滚到底。
$script:longText = (@(1..80 | ForEach-Object { "长回复第 $_ 行：岛仔 E2E 结果层滚动验收占位正文，用来撑满正文区并验证滚轮到底不会翻岛页。" }) + @('E2E-LONG-TAIL-MARK 这是最后一行')) -join ([char]10)

function Get-ScenarioSpec([string]$name) {
  switch ($name) {
    'done-repro' { return [pscustomobject]@{ driver = 'done'; request = [ordered]@{ text = $doneText }; options = @() } }
    # 选项用 OCR 友好且互不相同的短词（Windows OCR 会把「方案二」读成「方案一」，导致点错/判错）。
    'jump-repro' { return [pscustomobject]@{ driver = 'question-multi-turn'; request = [ordered]@{ options = @('冒烟测试', '完整回归'); question = '岛仔 E2E 多轮 1/2：请选择「冒烟测试」' }; options = @('冒烟测试') } }
    # 收起态提示卡上的跳转 chip（旧版是空实现）：详情卡先收起到 banner，再点 banner 上的 chip。
    'banner-jump-repro' { return [pscustomobject]@{ driver = 'question-multi-turn'; request = [ordered]@{ options = @('冒烟测试', '完整回归'); question = '岛仔 E2E 多轮 1/2：请选择「冒烟测试」' }; options = @('冒烟测试') } }
    # 长回复结果层滚动：content=2 才有正文层。
    'scroll-repro' { return [pscustomobject]@{ driver = 'done'; request = [ordered]@{ text = $script:longText }; options = @() } }
    default { return $null }
  }
}

# ---------------------------------------------------------------- 主流程

$spec = Get-ScenarioSpec $Scenario
if (-not $spec) { Fail 2 "未知场景 $Scenario" }

try {
  if (-not (Start-Island)) { Fail 3 '岛仔没有在 30 秒内发布桥接通道（检查 DSH 来源设置与 exe 能否启动）' }
  Start-Sleep -Milliseconds 900

  Set-EnvVar 'ELECTRON_RUN_AS_NODE' '1'
  Set-EnvVar 'DSH_HOME' (Join-Path $DataDir 'dsh-home')
  $profileDir = Join-Path $DataDir 'dsh-home\profiles\desktop'

  $scenarioStart = Get-Date
  $verdict = [pscustomobject]@{
    scenario  = $Scenario
    driver    = $spec.driver
    ok        = $false
    startedAt = $scenarioStart.ToString('s')
    steps     = @()
    evidence  = [ordered]@{}
    bridge    = $null
    error     = $null
    duration  = 0
  }

  Write-Step "===== 场景 $Scenario（驱动场景 $($spec.driver)，dwell=$Dwell）====="
  $shotBase = Join-Path $script:Shots $Scenario
  $stdout = Join-Path $script:Shots "$Scenario-driver.json"
  $stderr = Join-Path $script:Shots "$Scenario-driver.err.log"
  $jsonIn = Join-Path $script:Shots "$Scenario-input.json"
  [System.IO.File]::WriteAllText($jsonIn, ($spec.request | ConvertTo-Json -Depth 6), (New-Object System.Text.UTF8Encoding($false)))

  $traceBytesBefore = 0
  if (Test-Path $script:TracePath) { $traceBytesBefore = (Get-Item $script:TracePath).Length }
  $verdict.evidence['tracePath'] = $script:TracePath
  $verdict.evidence['traceBytesBefore'] = $traceBytesBefore

  $driverArgs = @($driveScript, '--profile', $profileDir, '--scenario', $spec.driver,
    '--wait', [string]($WaitSeconds * 1000), '--json-in', $jsonIn)
  $driver = Start-Process -FilePath $dshExe -ArgumentList $driverArgs -PassThru -WorkingDirectory $script:RepoRoot -RedirectStandardOutput $stdout -RedirectStandardError $stderr
  Note-Process '启动 agent-drive' $driver.Id $dshExe
  $verdict.evidence['driverPid'] = $driver.Id

  $region = Wait-IslandRegion 20
  if (-not $region) { $verdict.error = '20 秒内没有找到岛仔窗口' }

  if ($Scenario -eq 'done-repro') {
    # ---- 等完成卡
    $hit = Wait-Text -Region $region -Texts @('本轮已结束', '查看回复', '本轮已') -TimeoutSeconds 35
    $verdict.steps += [pscustomobject]@{ step = '等到完成通知卡'; ok = [bool]$hit; text = $(if ($hit) { $hit.text } else { $null }) }
    if (-not $hit) { $verdict.error = '35 秒内没有在岛上看到完成通知卡' }
    else {
      # 卡片出现后立刻抓一次：完成卡有 dwell 倒计时，动作要快。
      $dumpPre = Save-Capture -Region $region -Base "$shotBase-01-card"
      # content=0 时卡片应有一行说明，让用户点之前就知道里面没有正文。
      $cardHint = Find-InDump $dumpPre @('未接收回复正文', '仅状态')
      $verdict.evidence['cardHintText'] = $(if ($cardHint) { $cardHint.line } else { $null })
      $verdict.steps += [pscustomobject]@{ step = '完成卡摘要行显示「仅状态 · 未接收回复正文」'; ok = [bool]$cardHint; text = $(if ($cardHint) { $cardHint.line } else { $null }) }
      $btn = Find-InDump $dumpPre @('查看回复')
      if (-not $btn) { $btn = Find-TextSmart -Region $region -Texts @('查看回复') -Scales @(2, 4, 5, 6, 8) -Shot "$shotBase-02-button.png" -JsonOut "$shotBase-02-button-ocr.json" }
      if (-not $btn -and $ViewReplyPoint) {
        $pt = Parse-Point $ViewReplyPoint
        if ($pt) { $btn = [pscustomobject]@{ found = $true; center = $pt; text = '(手工坐标)'; line = $ViewReplyPoint; scale = 0 }; Write-Step "用 -ViewReplyPoint 兜底：$ViewReplyPoint" }
      }
      $verdict.steps += [pscustomobject]@{ step = 'OCR 定位「查看回复」按钮'; ok = [bool]$btn; center = $(if ($btn) { $btn.center } else { $null }); text = $(if ($btn) { $btn.text } else { $null }) }
      if (-not $btn) { $verdict.error = '没有定位到「查看回复」按钮（截图见 01/02）' }
      else {
        $fgBefore = Get-ForegroundMeta
        $verdict.evidence['foregroundBeforeClick'] = $fgBefore
        Write-Step ('点击前前台：' + ($fgBefore | ConvertTo-Json -Compress))
        $null = Click-Point $btn.center
        Start-Sleep -Milliseconds 900
        $dump1 = Save-Capture -Region $region -Base "$shotBase-03-after-0.9s"
        $m1 = Find-InDump $dump1 @('本轮回复', '本轮未接收', '复制内容', '可滚动阅读', '仅状态', '完成内容')
        Start-Sleep -Milliseconds 2400
        $dump2 = Save-Capture -Region $region -Base "$shotBase-04-after-3s"
        $m2 = Find-InDump $dump2 @('本轮回复', '本轮未接收', '复制内容', '可滚动阅读', '仅状态', '完成内容')
        $still = Find-InDump $dump2 @('查看回复')
        $fgAfter = Get-ForegroundMeta
        $verdict.evidence['foregroundAfterClick'] = $fgAfter
        $verdict.evidence['resultMarkersAfter09s'] = $(if ($m1) { $m1.line } else { $null })
        $verdict.evidence['resultMarkersAfter3s'] = $(if ($m2) { $m2.line } else { $null })
        $verdict.evidence['viewReplyStillVisible'] = [bool]$still
        # 说明层应只有「关闭」、没有「复制内容」；并给出设置路径提示。
        $verdict.evidence['resultHasCopyButton'] = [bool](Find-InDump $dump2 @('复制内容'))
        $verdict.evidence['resultHasSettingsHint'] = [bool](Find-InDump $dump2 @('完成内容', '设置', 'Agent'))
        $verdict.steps += [pscustomobject]@{ step = '点击后 0.9s 是否出现结果层'; ok = [bool]$m1; text = $(if ($m1) { $m1.line } else { $null }) }
        $verdict.steps += [pscustomobject]@{ step = '点击后 3s 是否出现结果层'; ok = [bool]$m2; text = $(if ($m2) { $m2.line } else { $null }) }
        $verdict.steps += [pscustomobject]@{ step = '点击后完成卡是否仍在（说明点击被静默吞掉）'; ok = [bool]$still; text = $(if ($still) { $still.line } else { $null }) }
        if (-not $m1 -and -not $m2) { Write-Step '结论迹象：点击「查看回复」后 3 秒内没有出现任何结果层文字（疑似死按钮）' }
        # ---- 对照组：点同一张卡片上的「关闭」必须立刻生效，证明点击链路本身是通的（不是点歪/被遮挡）
        $close = Find-InDump $dump2 @('关闭')
        if (-not $close) { $close = Find-TextSmart -Region $region -Texts @('关闭') -Scales @(2, 4, 6) -Shot "$shotBase-05-close.png" }
        $verdict.steps += [pscustomobject]@{ step = '对照组：OCR 定位「关闭」按钮'; ok = [bool]$close; center = $(if ($close) { $close.center } else { $null }) }
        if ($close) {
          $null = Click-Point $close.center
          Start-Sleep -Milliseconds 1200
          $dump3 = Save-Capture -Region $region -Base "$shotBase-06-after-close"
          $gone = (-not (Find-InDump $dump3 @('查看回复'))) -and (-not (Find-InDump $dump3 @('本轮已结束')))
          $verdict.evidence['cardGoneAfterClose'] = $gone
          $verdict.steps += [pscustomobject]@{ step = '对照组：点「关闭」后卡片消失（鼠标链路正常）'; ok = $gone }
        }
      }
    }

    # ---- 驱动结论（桥接侧）
    $exited = Wait-DriverExit -DriverPid $driver.Id -TimeoutSeconds 12
    if (-not $exited) { Stop-Process -Id $driver.Id -Force -ErrorAction SilentlyContinue; Write-Step '驱动未自行退出，已强制结束' }
    $json = $null
    $readDeadline = (Get-Date).AddSeconds(6)
    while ((Get-Date) -lt $readDeadline -and -not $json) { $json = Read-DriverJson $stdout; if (-not $json) { Start-Sleep -Milliseconds 200 } }
    $verdict.evidence['driverJson'] = $stdout
    if (-not $json) { if (-not $verdict.error) { $verdict.error = '驱动没有输出 JSON（见 *-driver.err.log）' } }
    else {
      $finished = @(@($json.bridge) | Where-Object { $_.kind -eq 'finished' })
      $entry = if ($finished.Count -ge 1) { $finished[0] } else { $null }
      $textLen = if ($entry -and $entry.sent -and $entry.sent.text) { ([string]$entry.sent.text).Length } else { 0 }
      $done = if ($entry -and $entry.reply) { [bool]$entry.reply.done } else { $false }
      $verdict.bridge = [pscustomobject]@{
        decision = $json.decision; ok = $json.ok; mode = $(if ($entry) { $entry.mode } else { $null })
        textLen = $textLen; done = $done; error = $json.error
      }
      $verdict.evidence['bridgeExpectation'] = 'content=0 时 mode 应为 0 且正文为空；桥接仍必须回 done:true'
      $verdict.steps += [pscustomobject]@{ step = '桥接确认完成事件（content=0 仍应回 done）'; ok = ($json.decision -eq 'done' -and $done); text = ("decision=$($json.decision) mode=$($entry.mode) textLen=$textLen done=$done") }
      if ($json.decision -ne 'done' -or -not $done) { if (-not $verdict.error) { $verdict.error = '桥接没有确认完成事件' } }
    }
  }

  if ($Scenario -eq 'jump-repro') {
    # ---- 等提问卡
    $hit = Wait-Text -Region $region -Texts @('需要你回答', '冒烟测试') -TimeoutSeconds 35
    $verdict.steps += [pscustomobject]@{ step = '等到提问卡'; ok = [bool]$hit; text = $(if ($hit) { $hit.text } else { $null }) }
    if (-not $hit) { $verdict.error = '35 秒内没有在岛上看到提问卡' }
    else {
      Start-Sleep -Milliseconds 600
      $dump = Save-Capture -Region $region -Base "$shotBase-01-question"
      $opt = Find-InDump $dump @('冒烟测试')
      if (-not $opt) {
        # 收起态：先点「查看并回答」打开详情
        $open = Find-InDump $dump @('查看并回答', '查看并', '查看')
        if (-not $open) { $open = Find-TextSmart -Region $region -Texts @('查看并回答', '查看并', '查看') -Scales @(2, 4, 6) -Shot "$shotBase-01b-open.png" }
        if ($open) {
          $verdict.steps += [pscustomobject]@{ step = '点开提问详情'; ok = $true; center = $open.center }
          $null = Click-Point $open.center
          Start-Sleep -Milliseconds 900
          $dump = Save-Capture -Region $region -Base "$shotBase-02-detail"
          $opt = Find-InDump $dump @('冒烟测试')
        }
      }
      $verdict.steps += [pscustomobject]@{ step = '提问详情卡已展开（看到选项「冒烟测试」）'; ok = [bool]$opt; text = $(if ($opt) { $opt.text } else { $null }) }

      # ---- 找「切换到客户端」chip（头部右上，META 小字号）
      # 关键词要限制在「头部 chip」：底部「回客户端」按钮也含「客户端」，OCR 会把 chip 读成「切客户端」。
      # 因此优先匹配含「切」的写法，兜底匹配「客户端」时取最靠上的一条，并校验落在标题头带（卡片顶部 140px 内）。
      $chip = Find-TextSmart -Region $region -Texts @('切客户端', '切换到客户端', '切换') -Scales @(4, 6, 3, 5, 8) -Shot "$shotBase-03-chip-search.png" -JsonOut "$shotBase-03-chip-search-ocr.json" -PreferTop
      if (-not $chip) {
        $anyClient = Find-TextSmart -Region $region -Texts @('客户端') -Scales @(4, 6, 3) -PreferTop
        if ($anyClient -and ($anyClient.center[1] - $region.top) -le 140) { $chip = $anyClient }
        elseif ($anyClient) { Write-Step ("OCR 只找到非头部的「客户端」（y=$($anyClient.center[1])），按键名不匹配处理") }
      }
      if (-not $chip -and $ChipPoint) {
        $pt = Parse-Point $ChipPoint
        if ($pt) { $chip = [pscustomobject]@{ found = $true; center = $pt; text = '(手工坐标)'; line = $ChipPoint; scale = 0 }; Write-Step "用 -ChipPoint 兜底：$ChipPoint" }
      }
      $verdict.steps += [pscustomobject]@{ step = 'OCR 定位头部「切换到客户端」chip'; ok = [bool]$chip; center = $(if ($chip) { $chip.center } else { $null }); text = $(if ($chip) { $chip.text } else { $null }) }
      if (-not $chip) { $verdict.error = '没有定位到「切换到客户端」chip（截图见 03）' }
      else {
        if ($DshFront) {
          $front = Set-DshForeground
          Start-Sleep -Milliseconds 700
          Write-Step "已主动把 DSH 置前：$front"
          $verdict.evidence['preAction'] = 'DshFront: 点击前把 DSH 置前'
        }
        if ($DshBack) {
          $fgNow = Ensure-DshNotForeground
          Write-Step ('已确保 DSH 不在前台，当前前台：' + ($fgNow | ConvertTo-Json -Compress))
          $verdict.evidence['preAction'] = 'DshBack: 点击前确保 DSH 不在前台'
        }
        if (-not $DshFront -and -not $DshBack) { $verdict.evidence['preAction'] = '未干预前台' }

        $regionBefore = Get-IslandRegion
        $fgBefore = Get-ForegroundMeta
        $verdict.evidence['islandRegion'] = $regionBefore
        $verdict.evidence['foregroundBeforeClick'] = $fgBefore
        Write-Step ('点击前前台：' + ($fgBefore | ConvertTo-Json -Compress))
        $verdict.steps += [pscustomobject]@{ step = '记录点击前前台窗口'; ok = $true; text = ("$($fgBefore.process) | $($fgBefore.title)") }

        if (-not $NoClick) {
          $null = Click-Point $chip.center
          Start-Sleep -Milliseconds 1500
          $fg1 = Get-ForegroundMeta
          $dump1 = Save-Capture -Region $regionBefore -Base "$shotBase-04-after-1.5s"
          $toast1 = Find-InDump $dump1 @('最前台', '已切到', '已请求', '仍在后台', '切到 DeepSeek', 'DeepSeek Harness 已')
          Start-Sleep -Milliseconds 1800
          $fg2 = Get-ForegroundMeta
          $dump2 = Save-Capture -Region $regionBefore -Base "$shotBase-05-after-3.3s"
          $toast2 = Find-InDump $dump2 @('最前台', '已切到', '已请求', '仍在后台', '切到 DeepSeek', 'DeepSeek Harness 已')
          $verdict.evidence['foregroundAfter1_5s'] = $fg1
          $verdict.evidence['foregroundAfter3_3s'] = $fg2
          $verdict.evidence['toastAfter1_5s'] = $(if ($toast1) { $toast1.line } else { $null })
          $verdict.evidence['toastAfter3_3s'] = $(if ($toast2) { $toast2.line } else { $null })
          $verdict.steps += [pscustomobject]@{ step = '点击后 1.5s 前台窗口'; ok = $true; text = ("$($fg1.process) | $($fg1.title)") }
          $verdict.steps += [pscustomobject]@{ step = '点击后 3.3s 前台窗口'; ok = $true; text = ("$($fg2.process) | $($fg2.title)") }
          $verdict.steps += [pscustomobject]@{ step = '点击后是否出现 toast/可见反馈'; ok = ([bool]$toast1 -or [bool]$toast2); text = $(if ($toast1) { $toast1.line } elseif ($toast2) { $toast2.line } else { $null }) }
        }

        # ---- 把 2 轮问题答完（验证点完 chip 之后卡片仍可用、桥接仍拿到答案）
        $clickPlan = @(
          @{ step = '点击第 1 题选项「冒烟测试」'; texts = @('冒烟测试'); base = '06-answer1'; verify = @('已答 1', '已答1'); footerRight = $false },
          @{ step = '点击「下一题」'; texts = @('下一题'); base = '07-next'; verify = @(); footerRight = $true },
          @{ step = '点击第 2 题选项「确认一」'; texts = @('确认一'); base = '08-answer2'; verify = @('已答 2', '已答2'); footerRight = $false },
          @{ step = '点击「检查答案」'; texts = @('检查答案'); base = '09-review'; verify = @(); footerRight = $true }
        )
        $okFlow = $true
        foreach ($plan in $clickPlan) {
          if (-not $okFlow) { break }
          $target = $null
          $deadline = (Get-Date).AddSeconds(12)
          # 选项行用「只比选项词多一个序号徽标」的过滤（取消题干里同名选项的干扰），
          # 并把 5/6 倍率纳入候选——实测小字号下选项行常常只有高倍率才读得出来。
          $planScales = if ($plan.footerRight) { @(3, 4, 2) } else { @(3, 4, 5, 6, 2) }
          $planMaxExtra = if ($plan.footerRight) { -1 } else { 2 }
          while ((Get-Date) -lt $deadline -and -not $target) {
            # 3 倍率优先：同一份 OCR 能同时读到正文和选项，PreferBottom 才会选中真正的选项行
            #（2 倍率常常只读到正文里的「方案一」，结果点在标题上）。
            $target = Find-TextSmart -Region (Get-IslandRegion) -Texts $plan.texts -Scales $planScales -PreferBottom -MaxExtra $planMaxExtra
            if (-not $target) { Start-Sleep -Milliseconds 400 }
          }
          if (-not $target -and $plan.footerRight) {
            # 禁用态按钮是暗灰字，OCR 读不到：按卡片右下角按钮的几何位置兜底。
            $pt = Get-FooterRightPoint (Get-IslandRegion)
            if ($pt) { $target = [pscustomobject]@{ found = $true; center = $pt; text = '(footer-geometry)'; scale = 0 }; Write-Step "按钮 OCR 未命中，按几何位置兜底 ($($pt -join ','))" }
          }
          $verdict.steps += [pscustomobject]@{ step = $plan.step; ok = [bool]$target; center = $(if ($target) { $target.center } else { $null }); text = $(if ($target) { $target.text } else { $null }) }
          if (-not $target) { $okFlow = $false; break }
          $null = Click-Point $target.center
          Start-Sleep -Milliseconds 800
          $selDump = Save-Capture -Region (Get-IslandRegion) -Base "$shotBase-$($plan.base)"
          if ($plan.verify -and $plan.verify.Count) {
            $selOk = Find-InDump $selDump $plan.verify
            $verdict.evidence["selection_" + $plan.base] = [bool]$selOk
            $verdict.steps += [pscustomobject]@{ step = ($plan.step + '（选中态已生效）'); ok = [bool]$selOk; text = $(if ($selOk) { $selOk.line } else { $null }) }
            if (-not $selOk) { $okFlow = $false; Write-Step '选项点击后没有看到「已答」变化，终止作答流程' }
          }
          if ($plan.texts -contains '检查答案' -and $target.text -eq '检查答案') {
            $confirm = $null
            $deadline2 = (Get-Date).AddSeconds(12)
            while ((Get-Date) -lt $deadline2 -and -not $confirm) {
              $confirm = Find-TextSmart -Region (Get-IslandRegion) -Texts @('确认提交') -Scales @(3, 4, 2) -PreferBottom
              if (-not $confirm) { Start-Sleep -Milliseconds 400 }
            }
            $verdict.steps += [pscustomobject]@{ step = '审阅页点「确认提交」'; ok = [bool]$confirm; center = $(if ($confirm) { $confirm.center } else { $null }) }
            if ($confirm) { $null = Click-Point $confirm.center; Start-Sleep -Milliseconds 900; $null = Save-Capture -Region (Get-IslandRegion) -Base "$shotBase-10-submitted" }
            else { $okFlow = $false }
          }
        }
        if (-not $okFlow) { Write-Step '注意：点击 chip 之后的问题作答流程没有走完（卡片可能已不可用）' }

        # ---- 驱动结论
        $exited = Wait-DriverExit -DriverPid $driver.Id -TimeoutSeconds 20
        if (-not $exited) { Stop-Process -Id $driver.Id -Force -ErrorAction SilentlyContinue; Write-Step '驱动未自行退出，已强制结束' }
        $json = $null
        $readDeadline = (Get-Date).AddSeconds(6)
        while ((Get-Date) -lt $readDeadline -and -not $json) { $json = Read-DriverJson $stdout; if (-not $json) { Start-Sleep -Milliseconds 200 } }
        $verdict.evidence['driverJson'] = $stdout
        if (-not $json) { if (-not $verdict.error) { $verdict.error = '驱动没有输出 JSON（见 *-driver.err.log）' } }
        else {
          $answers = @()
          if ($json.decision -and $json.decision.answers) { $answers = @($json.decision.answers) }
          $first = if ($answers.Count -ge 1) { @($answers[0].selected) } else { @() }
          $verdict.bridge = [pscustomobject]@{
            decision = $json.decision; ok = $json.ok; handback = $json.handback
            answerCount = $answers.Count; firstSelected = $first; error = $json.error
          }
          $verdict.steps += [pscustomobject]@{ step = '桥接收到 2 轮答案（点 chip 之后卡片仍可作答）'; ok = ($json.ok -and $answers.Count -ge 2); text = ("answers=$($answers.Count) first=$($first -join ',') handback=$($json.handback)") }
          if (-not $json.ok -or $answers.Count -lt 2) { if (-not $verdict.error) { $verdict.error = '桥接没有收到 2 轮答案' } }
        }
      }
    }
  }

  if ($Scenario -eq 'scroll-repro') {
    $hit = Wait-Text -Region $region -Texts @('本轮已结束', '查看回复', '本轮已') -TimeoutSeconds 35
    $verdict.steps += [pscustomobject]@{ step = '等到完成通知卡（content=2 长回复）'; ok = [bool]$hit; text = $(if ($hit) { $hit.text } else { $null }) }
    if (-not $hit) { $verdict.error = '35 秒内没有在岛上看到完成通知卡' }
    else {
      $dumpPre = Save-Capture -Region $region -Base "$shotBase-01-card"
      $btn = Find-InDump $dumpPre @('查看回复')
      if (-not $btn) { $btn = Find-TextSmart -Region $region -Texts @('查看回复') -Scales @(2, 3, 4, 5, 6) -Shot "$shotBase-02-button.png" }
      $verdict.steps += [pscustomobject]@{ step = 'OCR 定位「查看回复」'; ok = [bool]$btn; center = $(if ($btn) { $btn.center } else { $null }) }
      if (-not $btn) { $verdict.error = '没有定位到「查看回复」按钮' }
      else {
        $null = Click-Point $btn.center
        Start-Sleep -Milliseconds 900
        $dump1 = Save-Capture -Region $region -Base "$shotBase-03-result"
        $opened = Find-InDump $dump1 @('本轮回复', '复制内容', '可滚动阅读')
        $verdict.steps += [pscustomobject]@{ step = '结果层已打开（本轮回复）'; ok = [bool]$opened; text = $(if ($opened) { $opened.line } else { $null }) }
        if (-not $opened) { $verdict.error = '点「查看回复」后没有出现结果层' }
        else {
          # 滚轮：光标放在卡片正文中部，向下滚 12 格
          $box = Get-CardBox $region
          if ($box) { $wx = [int](($box.left + $box.right) / 2); $wy = [int]($box.top + ($box.bottom - $box.top) * 0.55) }
          else { $wx = [int](($region.left + $region.right) / 2); $wy = 200 }
          # 长文 80 行：每格滚 3 行左右，12 格只到第 37 行，滚 40 格足够到底。
          for ($i = 0; $i -lt 40; $i++) { [DaozaiRepro]::Wheel($wx, $wy, -120); Start-Sleep -Milliseconds 90 }
          Start-Sleep -Milliseconds 500
          $dump2 = Save-Capture -Region $region -Base "$shotBase-04-scrolled"
          $tail = Find-InDump $dump2 @('E2E-LONG-TAIL-MARK', 'LONG-TAIL', '最后一行')
          $stillResult = Find-InDump $dump2 @('复制内容', '可滚动阅读', '关闭')
          $pageFlip = Find-InDump $dump2 @('天气', '日历', '番茄', '设置')
          $verdict.evidence['scrollWheelPoint'] = @($wx, $wy)
          $verdict.evidence['tailMarkerSeen'] = $(if ($tail) { $tail.line } else { $null })
          $verdict.evidence['stillResultAfterScroll'] = [bool]$stillResult
          $verdict.evidence['pageFlippedAfterScroll'] = [bool]$pageFlip
          $verdict.steps += [pscustomobject]@{ step = '滚到底看到末行标记 E2E-LONG-TAIL-MARK'; ok = [bool]$tail; text = $(if ($tail) { $tail.line } else { $null }) }
          $verdict.steps += [pscustomobject]@{ step = '滚动后仍在结果层（没翻岛页）'; ok = ([bool]$stillResult -and -not [bool]$pageFlip); text = ("result=" + [bool]$stillResult + " pageFlip=" + [bool]$pageFlip) }
        }
      }
    }
    $exited = Wait-DriverExit -DriverPid $driver.Id -TimeoutSeconds 12
    if (-not $exited) { Stop-Process -Id $driver.Id -Force -ErrorAction SilentlyContinue }
    $json = $null
    $readDeadline = (Get-Date).AddSeconds(6)
    while ((Get-Date) -lt $readDeadline -and -not $json) { $json = Read-DriverJson $stdout; if (-not $json) { Start-Sleep -Milliseconds 200 } }
    $verdict.evidence['driverJson'] = $stdout
    if ($json) {
      $finished = @(@($json.bridge) | Where-Object { $_.kind -eq 'finished' })
      $entry = if ($finished.Count -ge 1) { $finished[0] } else { $null }
      $textLen = if ($entry -and $entry.sent -and $entry.sent.text) { ([string]$entry.sent.text).Length } else { 0 }
      $verdict.bridge = [pscustomobject]@{ decision = $json.decision; mode = $(if ($entry) { $entry.mode } else { $null }); textLen = $textLen }
      $verdict.steps += [pscustomobject]@{ step = '桥接确认完成（content=2 应带正文）'; ok = ($json.decision -eq 'done' -and $entry.mode -eq 2 -and $textLen -gt 0); text = ("mode=$($entry.mode) textLen=$textLen") }
    }
  }

  if ($Scenario -eq 'banner-jump-repro') {
    $hit = Wait-Text -Region $region -Texts @('需要你回答', '冒烟测试') -TimeoutSeconds 35
    $verdict.steps += [pscustomobject]@{ step = '等到提问卡'; ok = [bool]$hit; text = $(if ($hit) { $hit.text } else { $null }) }
    if (-not $hit) { $verdict.error = '35 秒内没有在岛上看到提问卡' }
    else {
      Start-Sleep -Milliseconds 600
      $dump = Save-Capture -Region $region -Base "$shotBase-01-question"
      if (-not (Find-InDump $dump @('冒烟测试'))) {
        $open = Find-InDump $dump @('查看并回答', '查看并', '查看')
        if (-not $open) { $open = Find-TextSmart -Region $region -Texts @('查看并回答', '查看并', '查看') -Scales @(2, 4, 6) }
        if ($open) { $null = Click-Point $open.center; Start-Sleep -Milliseconds 900; $null = Save-Capture -Region $region -Base "$shotBase-01b-detail" }
      }
      # 点详情卡右上角的「收起」图标（22 逻辑像素、距卡片右边 35px、垂直居中于头部）
      $box = Get-CardBox $region
      $collapsed = $false
      if ($box) {
        $ccx = [int]($box.right - 35); $ccy = [int]($box.top + 22)
        $verdict.evidence['collapseIconPoint'] = @($ccx, $ccy)
        $null = Click-Point @($ccx, $ccy)
        $collapsed = $true
      }
      Start-Sleep -Milliseconds 900
      $dumpB = Save-Capture -Region $region -Base "$shotBase-02-banner"
      $banner = Find-InDump $dumpB @('查看并回答', '查看并审批', '需要你回答', '等待你审批')
      $verdict.steps += [pscustomobject]@{ step = '点收起图标后回到收起态提示卡'; ok = ([bool]$collapsed -and [bool]$banner); text = $(if ($banner) { $banner.line } else { $null }) }
      # banner 上的 chip 是 9.5px 蓝底 pill，Windows OCR 基本读不出（实测 4/6/8 倍都缺这一行）；
      # 而且底部「回客户端」按钮也含「客户端」。所以：OCR 只在「头部带 + 卡片右半」接受命中，
      # 否则按几何位置点（chip 右边缘=卡片右边-PAD，中心在卡片右侧 78px、头部 42px）。
      $chip = $null
      $ocrChip = Find-TextSmart -Region $region -Texts @('切客户端', '切换到客户端', '切换') -Scales @(6, 8, 4) -PreferTop
      if ($ocrChip -and $box) {
        $inHeader = ($ocrChip.center[1] - $box.top) -le 70
        $inRight = $ocrChip.center[0] -ge ($box.right - 240)
        if ($inHeader -and $inRight) { $chip = $ocrChip }
        else { Write-Step ("OCR 命中的「$($ocrChip.text)」不在头部区域（x=$($ocrChip.center[0]) y=$($ocrChip.center[1])），改用几何位置") }
      }
      elseif ($ocrChip) { $chip = $ocrChip }
      if (-not $chip -and $box) {
        $chip = [pscustomobject]@{ found = $true; center = @([int]($box.right - 78), [int]($box.top + 42)); text = '(几何位置)'; scale = 0 }
      }
      $verdict.steps += [pscustomobject]@{ step = '收起态提示卡上找到「切换到客户端」chip'; ok = [bool]$chip; center = $(if ($chip) { $chip.center } else { $null }); text = $(if ($chip) { $chip.text } else { $null }) }
      if (-not $chip) { if (-not $verdict.error) { $verdict.error = '收起态提示卡上没有定位到跳转 chip' } }
      else {
        if ($DshFront) { $null = Set-DshForeground; Start-Sleep -Milliseconds 700 }
        if ($DshBack) { $null = Ensure-DshNotForeground }
        $fgBefore = Get-ForegroundMeta
        $verdict.evidence['foregroundBeforeClick'] = $fgBefore
        Write-Step ('点击前前台：' + ($fgBefore | ConvertTo-Json -Compress))
        # 点之前确认这个点确实落在岛仔窗口里（避免点到桌面/别的窗口）
        $win = Get-IslandRegion
        $inside = $win -and ($chip.center[0] -ge $win.winLeft) -and ($chip.center[0] -le $win.winRight) -and ($chip.center[1] -ge $win.winTop) -and ($chip.center[1] -le $win.winBottom)
        $verdict.evidence['islandWindowAtClick'] = $win
        $verdict.evidence['clickPointInsideIsland'] = $inside
        $verdict.steps += [pscustomobject]@{ step = '点击坐标落在岛仔窗口内'; ok = [bool]$inside; center = $chip.center; text = ("rect=$($win.winLeft),$($win.winTop),$($win.winRight),$($win.winBottom)") }
        $null = Click-Point $chip.center
        Start-Sleep -Milliseconds 500
        $dump05 = Save-Capture -Region $region -Base "$shotBase-03-after-0.5s"
        $toast0 = Find-InDump $dump05 @('最前台', '已切到', '已请求', '仍在后台')
        $bannerStill = Find-InDump $dump05 @('查看并回答', '回客户端')
        $verdict.evidence['toastAfter0_5s'] = $(if ($toast0) { $toast0.line } else { $null })
        $verdict.evidence['bannerStillVisibleAfterClick'] = [bool]$bannerStill
        Start-Sleep -Milliseconds 1000
        $fg1 = Get-ForegroundMeta
        $dump1 = Save-Capture -Region $region -Base "$shotBase-04-after-1.5s"
        $toast1 = Find-InDump $dump1 @('最前台', '已切到', '已请求', '仍在后台')
        Start-Sleep -Milliseconds 1800
        $fg2 = Get-ForegroundMeta
        $dump2 = Save-Capture -Region $region -Base "$shotBase-05-after-3.3s"
        $toast2 = Find-InDump $dump2 @('最前台', '已切到', '已请求', '仍在后台')
        $verdict.evidence['foregroundAfter1_5s'] = $fg1
        $verdict.evidence['foregroundAfter3_3s'] = $fg2
        $verdict.evidence['toastAfter1_5s'] = $(if ($toast1) { $toast1.line } else { $null })
        $verdict.evidence['toastAfter3_3s'] = $(if ($toast2) { $toast2.line } else { $null })
        $verdict.steps += [pscustomobject]@{ step = '点击后前台窗口'; ok = $true; text = ("$($fg1.process) -> $($fg2.process)") }
        $verdict.steps += [pscustomobject]@{ step = '收起态提示卡跳转有可见结论（toast）'; ok = ([bool]$toast1 -or [bool]$toast2); text = $(if ($toast1) { $toast1.line } elseif ($toast2) { $toast2.line } else { $null }) }
      }
      # 本场景只验收起态 chip 的反馈，不做作答：直接收掉驱动（桥接侧答案在 jump-repro 已验）。
      Stop-Process -Id $driver.Id -Force -ErrorAction SilentlyContinue
      $verdict.evidence['note'] = 'banner-jump-repro 只验收起态 chip 反馈；驱动被主动结束'
    }
  }

  $verdict.duration = [Math]::Round(((Get-Date) - $scenarioStart).TotalSeconds, 1)
  $verdict.ok = ($null -eq $verdict.error)
  $traceOut = Save-Trace $shotBase
  $verdict.evidence['traceTail'] = $traceOut
  $script:Results += $verdict
  if (-not $verdict.ok) { $script:Failures += "$($Scenario): $($verdict.error)" }
  $null = Save-Capture -Region (Get-IslandRegion) -Base "$shotBase-final"
} finally {
  if (-not $KeepAlive) { Stop-Island }
  Restore-Env
  [DaozaiReproPower]::Off()
}

$summary = [pscustomobject]@{
  ok        = ($script:Failures.Count -eq 0)
  at        = (Get-Date).ToString('s')
  scenario  = $Scenario
  exe       = $Exe
  exeHash   = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash
  runCopy   = $script:runExe
  dataDir   = $DataDir
  shots     = $script:Shots
  processes = $script:ProcessLog
  results   = $script:Results
  failures  = $script:Failures
  log       = $script:Log
}
Save-Summary $summary
Write-Host ''
Write-Host "截图与结果：$($script:Shots)"
Write-Host ($summary | ConvertTo-Json -Depth 8)
if ($script:Failures.Count) { exit 3 }
exit 0
