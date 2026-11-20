<#
岛仔真机操作小工具：对「正在运行的」岛仔窗口做 OCR 定位 + 真鼠标点击。
（阶段3 真实 DSH GUI 用；不启停岛仔，不做桥接校验。）

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-click-island.ps1 -Text '冒烟测试'
  powershell ... -Text '查看回复' -Shot 0001-pre.png -SaveDir <证据目录>
  powershell ... -Text '关闭' -Next      # -Next：不点击，只返回坐标
输出：一行 JSON { ok, found, center:[x,y], text, line, region, ocrLines }

注意：本文件必须 UTF-8 with BOM 保存。
#>
[CmdletBinding()]
param(
  [string]$Text,
  [string]$FindScales = '3,4,2,5,6,8',
  [string]$Shot,
  [string]$SaveDir,
  [switch]$Next,
  # 不能叫 $Pid：PowerShell 里 $PID 是只读自动变量。
  [int]$TargetPid = 0
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Threading;
public static class IslandClick {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr param);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  public static IntPtr FindWindowForProcess(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr hwnd, IntPtr p) {
      uint owner; GetWindowThreadProcessId(hwnd, out owner);
      if (owner != pid || !IsWindowVisible(hwnd)) return true;
      RECT r; if (!GetWindowRect(hwnd, out r)) return true;
      if (r.Right - r.Left < 120 || r.Bottom - r.Top < 20) return true;
      found = hwnd; return false;
    }, IntPtr.Zero);
    return found;
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

$ocrScript = Join-Path $PSScriptRoot 'agent-screen-ocr.ps1'
if (-not (Test-Path $ocrScript)) { Write-Output '{"ok":false,"error":"missing agent-screen-ocr.ps1"}'; exit 2 }
if ($SaveDir) { New-Item -ItemType Directory -Force -Path $SaveDir | Out-Null }

if (-not $TargetPid) {
  $proc = @(Get-Process -Name 'daozai' -ErrorAction SilentlyContinue)
  if (-not $proc.Count) { $proc = @(Get-Process -Name 'daozai-e2e' -ErrorAction SilentlyContinue) }
  if (-not $proc.Count) { Write-Output '{"ok":false,"error":"no daozai process"}'; exit 2 }
  $TargetPid = $proc[0].Id
}
$hwnd = [IslandClick]::FindWindowForProcess([uint32]$TargetPid)
if ($hwnd -eq [IntPtr]::Zero) { Write-Output '{"ok":false,"error":"no island window"}'; exit 2 }
$rect = New-Object IslandClick+RECT
if (-not [IslandClick]::GetWindowRect($hwnd, [ref]$rect)) { Write-Output '{"ok":false,"error":"GetWindowRect failed"}'; exit 2 }
$screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
$region = [pscustomobject]@{
  left   = [Math]::Max($screen.Left, $rect.Left - 12)
  right  = [Math]::Min($screen.Right, $rect.Right + 12)
  top    = [Math]::Max($screen.Top, $rect.Top - 6)
  bottom = [Math]::Min([Math]::Min($screen.Bottom, 800), $rect.Bottom + 60)
}
$regionText = "$($region.left),$($region.top),$($region.right),$($region.bottom)"

$cli = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $ocrScript, '-Region', $regionText, '-Find', $Text, '-FindScales', $FindScales)
$savePath = $null
if ($Shot) {
  $savePath = if ($SaveDir) { Join-Path $SaveDir $Shot } else { $Shot }
  $cli += @('-SaveImage', $savePath)
}
$previous = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try { $raw = & powershell @cli 2>&1 | Out-String } finally { $ErrorActionPreference = $previous }
$dump = $null
try { $dump = $raw | ConvertFrom-Json } catch { Write-Output ('{"ok":false,"error":"ocr parse failed"}'); exit 2 }

$found = $dump.find -and $dump.find.found
$result = [pscustomobject]@{
  ok       = $true
  found    = [bool]$found
  text     = $Text
  center   = $(if ($found) { $dump.find.center } else { $null })
  line     = $(if ($found) { $dump.find.line } else { $null })
  region   = $regionText
  shot     = $savePath
  ocrLines = @($dump.lines).Count
  pid      = $TargetPid
}
if ($found -and -not $Next) {
  $x = [int]$dump.find.center[0]; $y = [int]$dump.find.center[1]
  $result | Add-Member -NotePropertyName clicked -NotePropertyValue ([IslandClick]::Click($x, $y))
}
Write-Output ($result | ConvertTo-Json -Compress -Depth 4)
exit 0
