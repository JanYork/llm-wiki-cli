param([string]$Executable = (Get-Command lwc -ErrorAction Stop).Source)
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$taskName = 'LWC-Team-Sync'
# Per-Agent services retain only the path to the private credential file.
$credentialPath = $env:LWC_TEAM_CREDENTIALS_FILE
$arguments = 'space supervise'
if ($credentialPath) {
    $credentialPath = (Resolve-Path -LiteralPath $credentialPath).Path
    $hasher = [Security.Cryptography.SHA256]::Create()
    try { $hash = [BitConverter]::ToString($hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($credentialPath))).Replace('-', '').Substring(0,16) } finally { $hasher.Dispose() }
    $taskName += '-' + $hash
    $directory = Join-Path $env:LOCALAPPDATA 'LWC\services'
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $launcher = Join-Path $directory ($taskName + '.ps1')
    $escapedPath = $credentialPath.Replace("'", "''")
    $escapedExe = $Executable.Replace("'", "''")
    Set-Content -LiteralPath $launcher -Value "`$env:LWC_TEAM_CREDENTIALS_FILE = '$escapedPath'`n& '$escapedExe' space supervise`nexit `$LASTEXITCODE" -Encoding utf8
    $arguments = '-NoProfile -NonInteractive -File "' + $launcher + '"'
    $Executable = (Get-Command powershell.exe -ErrorAction Stop).Source
}
$action = New-ScheduledTaskAction -Execute $Executable -Argument $arguments
$trigger = New-ScheduledTaskTrigger -AtLogOn -User ([Security.Principal.WindowsIdentity]::GetCurrent().Name)
$principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
Write-Output "Installed $taskName for the current user."
