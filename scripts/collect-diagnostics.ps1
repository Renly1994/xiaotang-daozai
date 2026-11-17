param(
    [string]$OutputDirectory = [Environment]::GetFolderPath('Desktop'),
    [string]$DataDirectory = $(if ($env:DAOZAI_DATA_DIR) { $env:DAOZAI_DATA_DIR } else { Join-Path $env:APPDATA 'Daozai' })
)
$ErrorActionPreference = 'Stop'
$collectionRoot = Join-Path ([IO.Path]::GetTempPath()) ('daozai-diagnostics-' + [guid]::NewGuid().ToString('N'))
$utf8 = New-Object System.Text.UTF8Encoding($true)
$missing = New-Object 'System.Collections.Generic.List[string]'
function Save-Text([string]$Name, [string]$Value) {
    foreach ($key in @('LOCALAPPDATA', 'APPDATA', 'USERPROFILE')) {
        $privatePath = [Environment]::GetEnvironmentVariable($key)
        if ($privatePath) { $Value = $Value.Replace($privatePath, ('%' + $key + '%')) }
    }
    [IO.File]::WriteAllText((Join-Path $collectionRoot $Name), $Value, $utf8)
}
New-Item -ItemType Directory -Path $collectionRoot | Out-Null
try {
    $logs = @('last-error.log', 'last-graphics.log', 'last-startup.log', 'resources.log',
        'resources.previous.log', 'last-resource-anomaly.log', 'resource-guard.log')
    foreach ($name in $logs + @('glass-diagnostics.txt')) {
        $source = if ($name -eq 'glass-diagnostics.txt') { Join-Path $DataDirectory $name } else { Join-Path (Join-Path $DataDirectory 'diagnostics') $name }
        if (!(Test-Path -LiteralPath $source -PathType Leaf)) { $missing.Add($name); continue }
        try { Save-Text $name ([IO.File]::ReadAllText($source)) }
        catch { $missing.Add($name + '：无法读取') }
    }
    $info = [ordered]@{ CollectedAt = (Get-Date).ToString('o'); MissingLogs = $missing.ToArray() }
    try {
        $os = Get-CimInstance Win32_OperatingSystem -OperationTimeoutSec 10
        $info.System = [ordered]@{ Name = [string]$os.Caption; Version = [string]$os.Version; Build = [string]$os.BuildNumber; BootedAt = $os.LastBootUpTime.ToString('o') }
        $info.Graphics = @(Get-CimInstance Win32_VideoController -OperationTimeoutSec 10 | ForEach-Object {
            [pscustomobject]@{ Name = [string]$_.Name; Driver = [string]$_.DriverVersion; DriverDate = [string]$_.DriverDate }
        })
    } catch { $info.SystemQuery = '无法读取系统或显卡信息' }
    try {
        Add-Type -AssemblyName System.Windows.Forms
        $info.Screens = @([System.Windows.Forms.Screen]::AllScreens | ForEach-Object {
            [pscustomobject]@{ Device = $_.DeviceName; Primary = $_.Primary; X = $_.Bounds.X; Y = $_.Bounds.Y; Width = $_.Bounds.Width; Height = $_.Bounds.Height }
        })
    } catch { $info.ScreensQuery = '无法读取显示器信息' }
    $settingsPath = Join-Path $DataDirectory 'settings.txt'
    if (Test-Path -LiteralPath $settingsPath) {
        $info.DisplaySettings = @([IO.File]::ReadAllLines($settingsPath) | Where-Object { $_ -match '^(skin|island_shape|dock|glass_blur|island_size|island_offset|settings_theme|minimal|pet)=' })
    }
    try {
        Add-Type -TypeDefinition 'public static class DaozaiDiagnosticGui { [System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetGuiResources(System.IntPtr process, uint flags); }'
        $info.Processes = @(Get-Process -Name daozai -ErrorAction SilentlyContinue | ForEach-Object {
            $processInfo = [ordered]@{ Id = $_.Id; StartedAt = $_.StartTime.ToString('o'); Handles = $_.HandleCount; Threads = $_.Threads.Count; PrivateBytes = $_.PrivateMemorySize64; WorkingBytes = $_.WorkingSet64 }
            try {
                $processInfo.GdiObjects = [DaozaiDiagnosticGui]::GetGuiResources($_.Handle, 0)
                $processInfo.UserObjects = [DaozaiDiagnosticGui]::GetGuiResources($_.Handle, 1)
                $processInfo.Version = $_.MainModule.FileVersionInfo.FileVersion
                $processInfo.Sha256 = (Get-FileHash -LiteralPath $_.Path -Algorithm SHA256).Hash
            } catch { $processInfo.Details = '无法读取部分进程信息' }
            [pscustomobject]$processInfo
        })
    } catch { $info.ProcessQuery = '无法读取进程信息' }
    try {
        $faultQuery = "*[System[(EventID=1000 or EventID=1001 or EventID=1002) and TimeCreated[timediff(@SystemTime) <= 604800000]] and EventData[Data[@Name='AppName']='daozai.exe' or Data[@Name='P1']='daozai.exe']]"
        $info.ApplicationFaults = @(Get-WinEvent -LogName Application -FilterXPath $faultQuery -MaxEvents 30 -ErrorAction SilentlyContinue | ForEach-Object {
            $eventData = @{}
            foreach ($node in ([xml]$_.ToXml()).Event.EventData.Data) { if ($node.Name) { $eventData[[string]$node.Name] = $node.InnerText } }
            if ($eventData.AppName -ine 'daozai.exe' -and $eventData.P1 -ine 'daozai.exe') { return }
            $fault = [ordered]@{ At = $_.TimeCreated.ToString('o'); EventId = $_.Id; Provider = $_.ProviderName }
            foreach ($key in @('AppName', 'AppVersion', 'ModuleName', 'ModuleVersion', 'ExceptionCode', 'FaultingOffset', 'EventName', 'P1', 'P2', 'P3', 'P4', 'P5', 'P6', 'P7', 'P8')) {
                if ($eventData.ContainsKey($key)) { $fault[$key] = $eventData[$key] }
            }
            [pscustomobject]$fault
        })
        $info.DisplayResets = @(Get-WinEvent -FilterHashtable @{ LogName = 'System'; Id = 4101; StartTime = (Get-Date).AddDays(-7) } -MaxEvents 20 -ErrorAction SilentlyContinue | ForEach-Object {
            [pscustomobject]@{ At = $_.TimeCreated.ToString('o'); Provider = $_.ProviderName; EventId = $_.Id }
        })
    } catch { $info.EventQuery = '无法读取 Windows 事件记录' }
    Save-Text 'environment.json' ($info | ConvertTo-Json -Depth 7)
    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $archive = Join-Path $OutputDirectory ('岛仔诊断-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 6) + '.zip')
    Compress-Archive -Path (Join-Path $collectionRoot '*') -DestinationPath $archive
    Write-Output ('诊断包已保存：' + $archive)
    Write-Output '请将诊断包发给开发者，并说明异常出现时间、重启岛仔能否恢复、是否发生在睡眠唤醒后。'
    Write-Output '仅收集指定诊断日志、显示设置、系统与显卡信息及岛仔崩溃事件；不会自动上传。'
} finally {
    Get-ChildItem -LiteralPath $collectionRoot -File | ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force }
    Remove-Item -LiteralPath $collectionRoot -Force
}
