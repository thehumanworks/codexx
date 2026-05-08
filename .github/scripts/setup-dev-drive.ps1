# Configure a Dev Drive for Windows CI jobs.
#
# Try to create a Dev Drive VHD explicitly so Windows temp-heavy paths can use a
# trusted ReFS Dev Drive. Fall back to the runner-provided D: drive, then C:, if
# the runner image does not allow that provisioning path.

function Select-FallbackDrive {
    param([string]$Reason)

    if (Test-Path "D:\") {
        Write-Warning "$Reason Falling back to existing drive at D:"
        return "D:"
    } else {
        Write-Warning "$Reason Falling back to C:"
        return "C:"
    }
}

function Invoke-BestEffort {
    param([scriptblock]$Script, [string]$Description)

    try {
        & $Script
    } catch {
        Write-Warning "$Description failed: $($_.Exception.Message)"
    }
}

try {
    $VhdPath = Join-Path $env:RUNNER_TEMP "codex-dev-drive.vhdx"
    $SizeBytes = 64GB

    if (Test-Path $VhdPath) {
        Remove-Item -Path $VhdPath -Force
    }

    New-VHD -Path $VhdPath -SizeBytes $SizeBytes -Dynamic -ErrorAction Stop | Out-Null
    $Mounted = Mount-VHD -Path $VhdPath -Passthru -ErrorAction Stop
    $Disk = $Mounted | Get-Disk -ErrorAction Stop
    $Disk | Initialize-Disk -PartitionStyle GPT -ErrorAction Stop
    $Partition = $Disk | New-Partition -AssignDriveLetter -UseMaximumSize -ErrorAction Stop
    $Volume = $Partition | Format-Volume -FileSystem ReFS -NewFileSystemLabel "CodexDevDrive" -DevDrive -Confirm:$false -Force -ErrorAction Stop

    $Drive = "$($Volume.DriveLetter):"

    Invoke-BestEffort { fsutil devdrv trust $Drive } "Trusting Dev Drive $Drive"
    Invoke-BestEffort { fsutil devdrv enable /disallowAv } "Disabling AV filter attachment for Dev Drives"
    try {
        Dismount-VHD -Path $VhdPath
        Mount-VHD -Path $VhdPath | Out-Null
    } catch {
        Write-Warning "Remounting Dev Drive $Drive failed: $($_.Exception.Message)"
        if (-not (Test-Path "$Drive\")) {
            throw
        }
    }
    Invoke-BestEffort { fsutil devdrv query $Drive } "Querying Dev Drive $Drive"

    Write-Output "Using Dev Drive at $Drive"
} catch {
    $Drive = Select-FallbackDrive "Failed to create Dev Drive: $($_.Exception.Message)"
    Invoke-BestEffort { fsutil devdrv query $Drive } "Querying fallback drive $Drive"
}

$Tmp = "$Drive\codex-tmp"
New-Item -Path $Tmp -ItemType Directory -Force | Out-Null

$CargoTargetDir = "$Drive\codex-cargo-target"
New-Item -Path $CargoTargetDir -ItemType Directory -Force | Out-Null

@(
    "DEV_DRIVE=$Drive"
    "CARGO_TARGET_DIR=$CargoTargetDir"
    "TMP=$Tmp"
    "TEMP=$Tmp"
) | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
