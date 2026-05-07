# Configure a fast drive for Windows CI jobs.
#
# If the runner already exposes `D:`, prefer it directly. Otherwise fall back
# to `C:` without trying to provision a new volume in CI.

if (Test-Path "D:\") {
    Write-Output "Using existing drive at D:"
    $Drive = "D:"
} else {
    Write-Warning "No D: drive available; falling back to C:"
    $Drive = "C:"
}

$Tmp = "$Drive\codex-tmp"
New-Item -Path $Tmp -ItemType Directory -Force | Out-Null

@(
    "DEV_DRIVE=$Drive"
    "TMP=$Tmp"
    "TEMP=$Tmp"
) | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
