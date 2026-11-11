param(
    [ValidateRange(1, 10)][int]$Samples = 1,
    [ValidateRange(1, 10)][int]$IntervalSeconds = 2
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$awaitMethod = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and
    $_.GetGenericArguments().Count -eq 1 -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
} | Select-Object -First 1

function Wait-MediaResult($Operation, [Type]$ResultType) {
    $task = $awaitMethod.MakeGenericMethod($ResultType).Invoke($null, @($Operation))
    if (-not $task.Wait(5000)) { throw 'SMTC request timed out after five seconds' }
    $task.Result
}

$manager = Wait-MediaResult ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])
$sessions = @(
    for ($sample = 1; $sample -le $Samples; $sample++) {
    foreach ($session in $manager.GetSessions()) {
        try {
            $properties = Wait-MediaResult ($session.TryGetMediaPropertiesAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties])
            $playback = $session.GetPlaybackInfo()
            $timeline = $session.GetTimelineProperties()
            [PSCustomObject]@{
                Sample = $sample
                CapturedAt = [DateTimeOffset]::Now.ToString('o')
                Source = $session.SourceAppUserModelId
                Title = $properties.Title
                Artist = $properties.Artist
                Album = $properties.AlbumTitle
                MediaType = [string]$properties.PlaybackType
                PlaybackType = [string]$playback.PlaybackType
                Status = [string]$playback.PlaybackStatus
                PositionSeconds = $timeline.Position.TotalSeconds
                DurationSeconds = $timeline.EndTime.TotalSeconds
                TimelineUpdatedAt = [string]$timeline.LastUpdatedTime
                StartSeconds = $timeline.StartTime.TotalSeconds
                MinSeekSeconds = $timeline.MinSeekTime.TotalSeconds
                MaxSeekSeconds = $timeline.MaxSeekTime.TotalSeconds
            }
        } catch {
            [PSCustomObject]@{ Source = $session.SourceAppUserModelId; Error = $_.Exception.Message }
        }
    }
    if ($sample -lt $Samples) { Start-Sleep -Seconds $IntervalSeconds }
    }
)
ConvertTo-Json -InputObject $sessions -Depth 4
