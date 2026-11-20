<#
不经过 windows-MCP 的键盘输入：把文字直接打进 DSH 桌面端窗口（SendInput Unicode）。
用途：阶段3 验证「完成卡 → 查看回复」时，必须避免 windows-MCP 的「AI is controlling this computer」
悬浮层盖住岛仔卡片（该层只在 MCP 控制期间出现，OCR 会读不到按钮）。

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dsh-type.ps1 -Text '请只回复：收到'
  ... -Text '' -Enter        # 只按回车（提交已输入的内容）
注意：本文件必须 UTF-8 with BOM 保存。
#>
[CmdletBinding()]
param(
  [string]$Text = '',
  [switch]$Enter,
  [string]$ProcessName = 'DeepSeek Harness'
)

$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Threading;
public static class DshType {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr param);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool on);

  [StructLayout(LayoutKind.Sequential)]
  public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit)]
  public struct INPUTUNION { [FieldOffset(0)] public KEYBDINPUT ki; }
  [StructLayout(LayoutKind.Sequential)]
  public struct INPUT { public uint type; public INPUTUNION u; }
  [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);

  public static IntPtr FindLargestWindowAmong(uint[] pids) {
    IntPtr best = IntPtr.Zero; long bestArea = 0;
    EnumWindows(delegate(IntPtr hwnd, IntPtr param) {
      uint owner; GetWindowThreadProcessId(hwnd, out owner);
      bool match = false;
      for (int i = 0; i < pids.Length; i++) { if (pids[i] == owner) { match = true; break; } }
      if (!match || !IsWindowVisible(hwnd)) return true;
      RECT r; if (!GetWindowRect(hwnd, out r)) return true;
      long w = r.Right - r.Left, h = r.Bottom - r.Top;
      if (w < 400 || h < 300) return true;
      if (w * h > bestArea) { bestArea = w * h; best = hwnd; }
      return true;
    }, IntPtr.Zero);
    return best;
  }
  public static bool ForceForeground(IntPtr hwnd) {
    if (hwnd == IntPtr.Zero) return false;
    if (IsIconic(hwnd)) ShowWindow(hwnd, 9);
    IntPtr fg = GetForegroundWindow();
    uint fgPid; uint fgThread = GetWindowThreadProcessId(fg, out fgPid);
    uint me = GetCurrentThreadId();
    bool attached = false;
    if (fgThread != 0 && fgThread != me) attached = AttachThreadInput(me, fgThread, true);
    bool ok = SetForegroundWindow(hwnd);
    if (!ok) { BringWindowToTop(hwnd); ok = SetForegroundWindow(hwnd); }
    if (attached) AttachThreadInput(me, fgThread, false);
    return ok;
  }
  static void SendVk(ushort vk, bool up) {
    INPUT inp = new INPUT();
    inp.type = 1;
    inp.u.ki.wVk = vk;
    inp.u.ki.dwFlags = up ? 2u : 0u;
    SendInput(1, new INPUT[] { inp }, Marshal.SizeOf(typeof(INPUT)));
  }
  static void SendChar(char ch) {
    INPUT inp = new INPUT();
    inp.type = 1;
    inp.u.ki.wVk = 0;
    inp.u.ki.wScan = ch;
    inp.u.ki.dwFlags = 4u; // KEYEVENTF_UNICODE
    SendInput(1, new INPUT[] { inp }, Marshal.SizeOf(typeof(INPUT)));
    inp.u.ki.dwFlags = 4u | 2u;
    SendInput(1, new INPUT[] { inp }, Marshal.SizeOf(typeof(INPUT)));
  }
  public static void TypeText(string text) {
    foreach (char ch in text) { SendChar(ch); Thread.Sleep(8); }
  }
  public static void PressEnter() { SendVk(0x0D, false); Thread.Sleep(20); SendVk(0x0D, true); }
}
'@

$pids = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
if (-not $pids.Count) { Write-Output '{"ok":false,"error":"process not found"}'; exit 2 }
$hwnd = [DshType]::FindLargestWindowAmong([uint32[]]$pids)
if ($hwnd -eq [IntPtr]::Zero) { Write-Output '{"ok":false,"error":"window not found"}'; exit 2 }
$focused = [DshType]::ForceForeground($hwnd)
Start-Sleep -Milliseconds 350
$typed = 0
if ($Text) { [DshType]::TypeText($Text); $typed = $Text.Length }
Start-Sleep -Milliseconds 120
if ($Enter) { [DshType]::PressEnter() }
[pscustomobject]@{ ok = $true; focused = $focused; typed = $typed; enter = [bool]$Enter; hwnd = $hwnd.ToInt64() } | ConvertTo-Json -Compress
exit 0
