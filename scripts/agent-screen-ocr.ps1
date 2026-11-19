<#
岛仔 Agent 界面 E2E 的屏幕识别工具。

egui 自绘窗口没有 UIA 文本，只能靠「截图 + Windows OCR」拿到按钮/正文文字与坐标。
本脚本用 Windows 10/11 自带的 Windows.Media.Ocr（本机离线、无网络、无第三方依赖），
输出 UTF-8 JSON，坐标统一为「屏幕绝对坐标」，可以直接喂给点击（mcp__windows__Click 或 SetCursorPos）。

用法（Windows PowerShell 5.1；用 pwsh 7 调用会自动转交给 powershell.exe）：

  # 全屏识别，打印 JSON
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-screen-ocr.ps1

  # 只识别一个屏幕矩形（左上右下），结果坐标仍然是屏幕绝对坐标
  ... -Region 700,0,1900,300

  # 找按钮文字并给出中心点击坐标（推荐配合 -FindScales，多倍率轮试）
  ... -Region 700,0,1900,300 -Find "允许一次" -FindScales 2,3,4,5,6

  # 识别已有 PNG（MCP 截图存盘的文件），便于离线复现
  ... -ImagePath docs\superpowers\research\agent-e2e-shots\approval.png -Find "拒绝"

  # 留下截图 + 结果 JSON（-CropSave 按识别到的文字包围盒再裁一次，避免把桌面内容带进验收文档）
  ... -Region 700,0,1900,300 -SaveImage out\shot.png -CropSave -OutFile out\ocr.json

为什么需要放大：岛仔卡片正文 11-16px，Windows OCR 在 1:1 像素下对中文识别率很差；
按整数倍最近邻放大后识别率明显提升，坐标除以倍数还原成屏幕像素。
同一按钮在不同倍率下成功率不同（短按钮 2 字用 6 倍更容易命中，长按钮 3-4 倍更稳），
所以 -FindScales 会依次尝试，命中即返回。

退出码：0 = 识别成功（-Find 时要求命中）；2 = 识别成功但 -Find 未命中；1 = 识别失败。

注意：本文件必须以 UTF-8 with BOM 保存，否则 Windows PowerShell 5.1 会按本地代码页
解析中文注释而报语法错误（修改后用 `Get-Content -Raw -Encoding UTF8` +
`[IO.File]::WriteAllText(..., UTF8Encoding($true))` 重新加 BOM）。
#>
[CmdletBinding()]
param(
  # -File 调用时 PowerShell 按字面量传参，所以用字符串接收 "left,top,right,bottom" 再解析。
  [string]$Region,
  [string]$ImagePath,
  [string]$Find,
  # 依次尝试的放大倍率，例如 "2,3,4,6"；给定后忽略 -Scale。
  [string]$FindScales,
  [int]$Scale = 1,
  [string]$Language,
  [string]$OutFile,
  # 保存本次截图（放大前、1:1 像素）的副本，便于把 OCR 结论留档。
  [string]$SaveImage,
  # 把 -SaveImage 再裁到本次 OCR 文字包围盒（±28px），避免把桌面内容带进验收截图。
  [switch]$CropSave,
  # 在 -CropSave 基础上再按“深色卡片像素”收边（岛仔默认深色皮肤），把透明区透出来的桌面裁掉。
  [switch]$CropCard,
  [switch]$Pretty
)

$ErrorActionPreference = 'Stop'

$RegionValues = @()
if ($Region) {
  $RegionValues = @($Region -split '[,;\s]+' | Where-Object { $_ -ne '' } | ForEach-Object { [int]$_ })
  if ($RegionValues.Count -ne 4) { throw '-Region 需要 4 个整数: left,top,right,bottom' }
}

# Windows.Media.Ocr 的投影只在 Windows PowerShell 5.1（.NET Framework）里可用；
# 在 pwsh 7 下自动转交给 powershell.exe，参数原样透传，避免调用方记两套用法。
if ($PSVersionTable.PSEdition -ne 'Desktop') {
  $exe = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
  $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $PSCommandPath)
  foreach ($kv in $PSBoundParameters.GetEnumerator()) {
    $name = "-$($kv.Key)"
    $value = $kv.Value
    if ($value -is [switch]) { if ($value.IsPresent) { $argv += $name } }
    elseif ($null -ne $value) { $argv += $name; $argv += [string]$value }
  }
  & $exe @argv
  exit $LASTEXITCODE
}

Add-Type -AssemblyName System.Runtime.WindowsRuntime
Add-Type -AssemblyName System.Drawing

# DPI：GDI 截图必须进程 DPI 感知，否则高缩放下截图是放大的、坐标对不上。
try {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class DaozaiOcrDpi {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
}
'@
  $null = [DaozaiOcrDpi]::SetProcessDpiAwareness(2)
} catch { try { $null = [DaozaiOcrDpi]::SetProcessDPIAware() } catch {} }

# WinRT 类型必须显式触达一次，PowerShell 才会生成投影。
$null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime]
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType = WindowsRuntime]

function Get-AwaitMethod {
  [System.WindowsRuntimeSystemExtensions].GetMethods() |
    Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
                   $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } |
    Select-Object -First 1
}
$script:AsTask = Get-AwaitMethod
function Await($operation, [Type]$resultType) {
  $task = $script:AsTask.MakeGenericMethod($resultType).Invoke($null, @($operation))
  $task.Wait(-1) | Out-Null
  $task.Result
}

function Save-ScreenPng($rect, [string]$path) {
  Add-Type -AssemblyName System.Windows.Forms
  if (-not $rect) { $rect = @(0, 0, [System.Windows.Forms.SystemInformation]::VirtualScreen.Width, [System.Windows.Forms.SystemInformation]::VirtualScreen.Height) }
  $x = [int]$rect[0]; $y = [int]$rect[1]; $w = [int]$rect[2] - [int]$rect[0]; $h = [int]$rect[3] - [int]$rect[1]
  if ($w -le 0 -or $h -le 0) { throw "截图矩形无效: $($rect -join ',')" }
  $bmp = New-Object System.Drawing.Bitmap($w, $h)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  try { $g.CopyFromScreen($x, $y, 0, 0, (New-Object System.Drawing.Size($w, $h))) }
  finally { $g.Dispose() }
  try { $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png) } finally { $bmp.Dispose() }
}

function Save-ScaledPng([string]$source, [string]$path, [int]$scale) {
  $img = [System.Drawing.Image]::FromFile($source)
  try {
    if ($scale -le 1) { Copy-Item -LiteralPath $source -Destination $path -Force; return }
    $big = New-Object System.Drawing.Bitmap(($img.Width * $scale), ($img.Height * $scale))
    $g = [System.Drawing.Graphics]::FromImage($big)
    try {
      $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
      $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
      $g.DrawImage($img, 0, 0, $big.Width, $big.Height)
    } finally { $g.Dispose() }
    try { $big.Save($path, [System.Drawing.Imaging.ImageFormat]::Png) } finally { $big.Dispose() }
  } finally { $img.Dispose() }
}

$script:Engine = $null
function Get-OcrEngine([string]$tag) {
  if ($script:Engine) { return $script:Engine }
  if ($tag) {
    try {
      $lang = New-Object Windows.Globalization.Language($tag)
      $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage($lang)
      if ($engine) { $script:Engine = $engine; return $engine }
    } catch {}
    throw "OCR 语言不可用: $tag（可用: $((Get-OcrLanguages) -join ', ')）"
  }
  $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
  if (-not $engine) { throw '系统没有可用的 OCR 语言包（设置 → 时间和语言 → 语言，添加中文识别）' }
  $script:Engine = $engine
  $engine
}
function Get-OcrLanguages {
  try { @([Windows.Media.Ocr.OcrEngine]::AvailableRecognizerLanguages | ForEach-Object { $_.LanguageTag }) } catch { @() }
}

function Invoke-Ocr([string]$file) {
  $storage = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($file)) ([Windows.Storage.StorageFile])
  $stream = Await ($storage.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
  try {
    $decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
    $bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
    $engine = Get-OcrEngine $Language
    $result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
  } finally { $stream.Dispose() }
  $result
}

# 行/词框：Windows OCR 的 BoundingRect 是相对（放大后）图片的像素，
# 除以放大倍数再加 region 偏移换成屏幕坐标。
# 注意：WinRT 集合（Lines/Words）不能可靠地按索引访问，一律先 @() 转成 .NET 数组。
function Convert-Box($box, [int]$dx, [int]$dy, [int]$scale) {
  $s = [Math]::Max(1, $scale)
  $x = [int]([double]$box.X / $s) + $dx
  $y = [int]([double]$box.Y / $s) + $dy
  $w = [int]([double]$box.Width / $s)
  $h = [int]([double]$box.Height / $s)
  [pscustomobject]@{ x = $x; y = $y; w = $w; h = $h; cx = $x + [int]($w / 2); cy = $y + [int]($h / 2) }
}

# 一次识别：返回行数组（含屏幕坐标）与 -Find 的命中结果。
function Invoke-OcrPass([string]$file, [int]$dx, [int]$dy, [int]$scale, [string]$needle) {
  $result = Invoke-Ocr $file
  $lines = @()
  foreach ($line in @($result.Lines)) {
    $words = @()
    $x1 = [int]::MaxValue; $y1 = [int]::MaxValue; $x2 = 0; $y2 = 0
    foreach ($word in @($line.Words)) {
      $b = Convert-Box $word.BoundingRect $dx $dy $scale
      if ($b.x -lt $x1) { $x1 = $b.x }; if ($b.y -lt $y1) { $y1 = $b.y }
      if (($b.x + $b.w) -gt $x2) { $x2 = $b.x + $b.w }; if (($b.y + $b.h) -gt $y2) { $y2 = $b.y + $b.h }
      $words += [pscustomobject]@{ text = [string]$word.Text; rect = @($b.x, $b.y, $b.w, $b.h); center = @($b.cx, $b.cy) }
    }
    if (-not $words.Count) { continue }
    $raw = [string]$line.Text
    $compact = ($raw -replace '\s+', '')
    if (-not $compact) { continue }
    $lineBox = [pscustomobject]@{ x = $x1; y = $y1; w = $x2 - $x1; h = $y2 - $y1; cx = $x1 + [int](($x2 - $x1) / 2); cy = $y1 + [int](($y2 - $y1) / 2) }
    $lines += [pscustomobject]@{ text = $raw; compact = $compact; rect = @($lineBox.x, $lineBox.y, $lineBox.w, $lineBox.h); center = @($lineBox.cx, $lineBox.cy); words = $words }
  }

  $findResult = $null
  if ($needle) {
    $target = ($needle -replace '\s+', '')
    $matches = @()
    foreach ($line in $lines) {
      $haystack = $line.compact
      $at = $haystack.IndexOf($target, [StringComparison]::OrdinalIgnoreCase)
      while ($at -ge 0) {
        # 行内按字符比例估算目标中心：中英文混排时够用，按钮高度更影响点击成功率。
        $ratio = $at / [Math]::Max(1, $haystack.Length)
        $lenRatio = $target.Length / [Math]::Max(1, $haystack.Length)
        $mx = $line.rect[0] + [int](($ratio + $lenRatio / 2) * $line.rect[2])
        $my = $line.center[1]
        $matches += [pscustomobject]@{ text = $line.text; center = @($mx, $my); rect = $line.rect }
        $at = $haystack.IndexOf($target, $at + $target.Length, [StringComparison]::OrdinalIgnoreCase)
      }
    }
    if ($matches.Count) {
      $findResult = [pscustomobject]@{ found = $true; query = $needle; center = $matches[0].center; line = $matches[0].text; matches = $matches }
    } else {
      $findResult = [pscustomobject]@{ found = $false; query = $needle; center = $null; line = $null; matches = @() }
    }
  }
  [pscustomobject]@{ lines = $lines; find = $findResult; text = [string]$result.Text }
}

$rawTemp = $null
$exit = 0
try {
  $dx = 0; $dy = 0
  if ($ImagePath) {
    $rawPath = (Resolve-Path -LiteralPath $ImagePath).Path
  } else {
    if (-not $RegionValues.Count) { $RegionValues = @(0, 0, 3440, 1440) }
    $rawTemp = Join-Path ([System.IO.Path]::GetTempPath()) ("daozai-ocr-" + [guid]::NewGuid().ToString('N') + '.png')
    Save-ScreenPng $RegionValues $rawTemp
    $rawPath = $rawTemp
  }
  if ($RegionValues.Count -eq 4) { $dx = [int]$RegionValues[0]; $dy = [int]$RegionValues[1]; $regionOut = $RegionValues }
  else { $regionOut = @($dx, $dy, $dx + 3440, $dy + 1440) }

  $saveTarget = $null
  if ($SaveImage) {
    $saveTarget = [System.IO.Path]::GetFullPath($SaveImage)
    $dir = Split-Path -Parent $saveTarget
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    Copy-Item -LiteralPath $rawPath -Destination $saveTarget -Force
  }

  $scales = @()
  if ($FindScales) { $scales = @($FindScales -split '[,;\s]+' | Where-Object { $_ -ne '' } | ForEach-Object { [Math]::Min(12, [Math]::Max(1, [int]$_)) }) }
  if (-not $scales.Count) { $scales = @([Math]::Min(12, [Math]::Max(1, $Scale))) }

  $attempts = @()
  $chosen = $null
  $bigCache = @{}
  foreach ($s in $scales) {
    if ($s -gt 1) {
      if (-not $bigCache.ContainsKey($s)) {
        $big = Join-Path ([System.IO.Path]::GetTempPath()) ("daozai-ocr-big-" + [guid]::NewGuid().ToString('N') + '.png')
        Save-ScaledPng $rawPath $big $s
        $bigCache[$s] = $big
      }
      $pass = Invoke-OcrPass $bigCache[$s] $dx $dy $s $Find
    } else {
      $pass = Invoke-OcrPass $rawPath $dx $dy $s $Find
    }
    $attempts += [pscustomobject]@{ scale = $s; lineCount = @($pass.lines).Count; found = ($null -ne $pass.find -and $pass.find.found) }
    if ($null -eq $chosen -or @($chosen.lines).Count -eq 0) { $chosen = $pass }
    if ($Find -and $null -ne $pass.find -and $pass.find.found) { $chosen = $pass; break }
  }
  foreach ($big in $bigCache.Values) { Remove-Item -LiteralPath $big -Force -ErrorAction SilentlyContinue }
  if ($saveTarget -and $CropSave -and @($chosen.lines).Count) {
    # 只留卡片正文区域：按 OCR 行的并集裁剪，±28px 余量。
    try {
      $minX = ($chosen.lines | ForEach-Object { $_.rect[0] } | Measure-Object -Minimum).Minimum
      $minY = ($chosen.lines | ForEach-Object { $_.rect[1] } | Measure-Object -Minimum).Minimum
      $maxX = ($chosen.lines | ForEach-Object { $_.rect[0] + $_.rect[2] } | Measure-Object -Maximum).Maximum
      $maxY = ($chosen.lines | ForEach-Object { $_.rect[1] + $_.rect[3] } | Measure-Object -Maximum).Maximum
      $img = [System.Drawing.Image]::FromFile($rawPath)
      try {
        $ix = [int]$minX - $dx - 28; $iy = [int]$minY - $dy - 28
        $iw = [int]($maxX - $minX) + 56; $ih = [int]($maxY - $minY) + 56
        if ($ix -lt 0) { $iw += $ix; $ix = 0 }
        if ($iy -lt 0) { $ih += $iy; $iy = 0 }
        if ($ix + $iw -gt $img.Width) { $iw = $img.Width - $ix }
        if ($iy + $ih -gt $img.Height) { $ih = $img.Height - $iy }
        if ($iw -gt 40 -and $ih -gt 40) {
          $crop = New-Object System.Drawing.Bitmap($iw, $ih)
          $g = [System.Drawing.Graphics]::FromImage($crop)
          try { $g.DrawImage($img, (New-Object System.Drawing.Rectangle(0, 0, $iw, $ih)), (New-Object System.Drawing.Rectangle($ix, $iy, $iw, $ih)), [System.Drawing.GraphicsUnit]::Pixel) }
          finally { $g.Dispose() }
          if ($CropCard) {
            # 岛仔默认深色皮肤：卡片是近黑圆角块，透明区透出桌面。
            # 按“绝大多数像素是深色”的行/列收边；检测失败就保持文字包围盒的结果。
            try {
              $dark = { param($c) (($c.R + $c.G + $c.B) -lt 150) }
              $rowDark = New-Object 'bool[]' $crop.Height
              for ($yy = 0; $yy -lt $crop.Height; $yy++) {
                $cnt = 0
                for ($xx = 0; $xx -lt $crop.Width; $xx += 4) { $c = $crop.GetPixel($xx, $yy); if (& $dark $c) { $cnt++ } }
                $rowDark[$yy] = ($cnt -ge [int](($crop.Width / 4) * 0.45))
              }
              # 行：从顶部找一段连续深色（允许 ≤12 行缺口），卡片一定从窗口顶部开始；
              # 不能直接用 min/max，否则底部任务栏这类深色行会把裁剪拉到整屏高。
              $rowEnd = -1; $gap = 0; $seen = $false
              for ($yy = 0; $yy -lt $crop.Height; $yy++) {
                if ($rowDark[$yy]) { $rowEnd = $yy; $gap = 0; $seen = $true }
                elseif ($seen) { $gap++; if ($gap -gt 12) { break } }
                elseif ($yy -gt 16) { break }
              }
              $rowStart = 0
              for ($yy = 0; $yy -lt $crop.Height; $yy++) { if ($rowDark[$yy]) { $rowStart = $yy; break } }
              # 列：只在上面找到的卡片行范围内统计，否则矮卡片（如完成通知）的列深色比例不够。
              $colDark = New-Object 'bool[]' $crop.Width
              $spanRows = [Math]::Max(1, $rowEnd - $rowStart + 1)
              for ($xx = 0; $xx -lt $crop.Width; $xx++) {
                $cnt = 0
                for ($yy = $rowStart; $yy -le $rowEnd; $yy += 2) { $c = $crop.GetPixel($xx, $yy); if (& $dark $c) { $cnt++ } }
                $colDark[$xx] = ($cnt -ge [int](($spanRows / 2) * 0.45))
              }
              $colStart = [int]($crop.Width / 2); $colEnd = $colStart
              while ($colStart -gt 0 -and $colDark[$colStart - 1]) { $colStart-- }
              while ($colEnd -lt ($crop.Width - 1) -and $colDark[$colEnd + 1]) { $colEnd++ }
              if ($rowEnd -gt 120 -and ($colEnd - $colStart) -gt 120) {
                $pad = 10
                $ny = [Math]::Max(0, $rowStart - $pad); $nh = [Math]::Min($crop.Height - $ny, ($rowEnd - $rowStart + 1) + $pad * 2)
                $nx = [Math]::Max(0, $colStart - $pad); $nw = [Math]::Min($crop.Width - $nx, ($colEnd - $colStart + 1) + $pad * 2)
                $tight = New-Object System.Drawing.Bitmap($nw, $nh)
                $g2 = [System.Drawing.Graphics]::FromImage($tight)
                try { $g2.DrawImage($crop, (New-Object System.Drawing.Rectangle(0, 0, $nw, $nh)), (New-Object System.Drawing.Rectangle($nx, $ny, $nw, $nh)), [System.Drawing.GraphicsUnit]::Pixel) }
                finally { $g2.Dispose() }
                $crop.Dispose()
                $crop = $tight
              }
            } catch { /* 收边失败就保持文字包围盒 */ }
          }
          $crop.Save($saveTarget, [System.Drawing.Imaging.ImageFormat]::Png)
          $crop.Dispose()
        }
      } finally { $img.Dispose() }
    } catch { /* 裁剪失败就保留整图，不影响识别结论 */ }
  }
  if ($null -eq $chosen) { throw '没有任何可用的放大倍率' }

  $findResult = $chosen.find
  if ($Find -and -not ($findResult -and $findResult.found)) { $exit = 2 }

  $json = [pscustomobject]@{
    ok        = $true
    source    = if ($ImagePath) { 'image' } else { 'screen' }
    region    = $regionOut
    language  = (Get-OcrEngine $Language).RecognizerLanguage.LanguageTag
    scales    = $scales
    attempts  = $attempts
    lineCount = @($chosen.lines).Count
    ocrText   = $chosen.text
    lines     = $chosen.lines
    find      = $findResult
  }
  $text = if ($Pretty) { $json | ConvertTo-Json -Depth 8 } else { $json | ConvertTo-Json -Depth 8 -Compress }
  if ($OutFile) {
    $dir = Split-Path -Parent $OutFile
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    [System.IO.File]::WriteAllText([System.IO.Path]::GetFullPath($OutFile), $text, (New-Object System.Text.UTF8Encoding($false)))
  }
  # 显式用 UTF-8 写 stdout：避免中文在重定向时按控制台代码页乱码。
  [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
  Write-Output $text
} catch {
  $err = [pscustomobject]@{ ok = $false; error = $_.Exception.Message; type = $_.Exception.GetType().Name; languages = @(Get-OcrLanguages) }
  [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
  Write-Output ($err | ConvertTo-Json -Depth 4 -Compress)
  $exit = 1
} finally {
  # 只删自己产生的临时文件（-ImagePath 传入的原图不动）。
  if ($rawTemp -and (Test-Path -LiteralPath $rawTemp)) { Remove-Item -LiteralPath $rawTemp -Force -ErrorAction SilentlyContinue }
}
exit $exit
