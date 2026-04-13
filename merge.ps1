# merge.ps1 - Copy .torrent files from first-level download directories into merge/

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$mergeDir = Join-Path $scriptDir "merge"
$skipDirs = @(".git", ".github", ".idea", ".claude", "src", "target", "frontend", "data", "merge")

if (-not (Test-Path $mergeDir)) {
    New-Item -ItemType Directory -Path $mergeDir | Out-Null
    Write-Host "Created: $mergeDir"
}

$totalCopied = 0
$totalSkipped = 0

$sourceDirs = Get-ChildItem -Path $scriptDir -Directory | Where-Object {
    $skipDirs -notcontains $_.Name
}

foreach ($dir in $sourceDirs) {
    $files = Get-ChildItem -Path $dir.FullName -File -Filter "*.torrent" -ErrorAction SilentlyContinue
    if ($null -eq $files -or $files.Count -eq 0) {
        continue
    }

    $copied = 0
    $skipped = 0
    foreach ($f in $files) {
        $dest = Join-Path $mergeDir $f.Name
        if ([System.IO.File]::Exists($dest)) {
            $skipped++
        } else {
            Copy-Item -LiteralPath $f.FullName -Destination $dest
            $copied++
        }
    }

    $totalCopied += $copied
    $totalSkipped += $skipped
    Write-Host "  $($dir.Name) : copied=$copied skipped=$skipped total=$($files.Count)"
}

Write-Host ""
Write-Host "Done. Copied: $totalCopied, Skipped (already exist): $totalSkipped, Destination: $mergeDir" -ForegroundColor Green
