param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Ports
)

if (-not $Ports -or $Ports.Count -eq 0) {
    Write-Host "Usage: .\kill-ports.ps1 <port1> <port2> ..." -ForegroundColor Yellow
    exit 1
}

$uniquePorts = @()
foreach ($port in $Ports) {
    if ($port -notmatch '^\d+$') {
        Write-Host "Invalid port: $port" -ForegroundColor Red
        exit 1
    }
    $portNumber = [int]$port
    if ($portNumber -lt 1 -or $portNumber -gt 65535) {
        Write-Host "Port out of range: $port" -ForegroundColor Red
        exit 1
    }
    if ($uniquePorts -notcontains $portNumber) {
        $uniquePorts += $portNumber
    }
}

$targetPids = @()

foreach ($port in $uniquePorts) {
    $tcp = @(Get-NetTCPConnection -LocalPort $port -ErrorAction SilentlyContinue)
    $udp = @(Get-NetUDPEndpoint -LocalPort $port -ErrorAction SilentlyContinue)
    $owners = @($tcp + $udp | ForEach-Object { [int]$_.OwningProcess } | Where-Object { $_ -gt 0 } | Select-Object -Unique)

    if ($owners.Count -eq 0) {
        Write-Host "Port ${port}: no process found" -ForegroundColor Yellow
        continue
    }

    foreach ($processId in $owners) {
        if ($targetPids -notcontains $processId) {
            $targetPids += $processId
        }
    }
}

if ($targetPids.Count -eq 0) {
    Write-Host "Nothing to stop." -ForegroundColor Yellow
    exit 0
}

foreach ($processId in $targetPids) {
    $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
    if ($null -eq $process) {
        Write-Host "PID ${processId}: already exited" -ForegroundColor Yellow
        continue
    }

    Write-Host "Stopping PID $processId ($($process.ProcessName))" -ForegroundColor Cyan
    Stop-Process -Id $processId -Force
}

Write-Host "Done." -ForegroundColor Green
