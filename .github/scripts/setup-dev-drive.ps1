# Configure a fast drive for Windows CI jobs.
#
# If the runner already exposes `D:`, prefer it directly. Otherwise create a
# small Dev Drive VHD and use that mount point for temp-heavy work.

if (Test-Path "D:\") {
    Write-Output "Using existing drive at D:"
    $Drive = "D:"
} else {
    try {
        $VhdPath = "C:/codex_dev_drive.vhdx"
        $Volume = New-VHD -Path $VhdPath -SizeBytes 20GB -ErrorAction Stop |
            Mount-VHD -Passthru -ErrorAction Stop |
            Initialize-Disk -Passthru -ErrorAction Stop |
            New-Partition -AssignDriveLetter -UseMaximumSize -ErrorAction Stop |
            Format-Volume -DevDrive -Confirm:$false -Force -ErrorAction Stop

        $Drive = "$($Volume.DriveLetter):"

        fsutil devdrv trust $Drive
        fsutil devdrv enable /disallowAv

        Dismount-VHD -Path $VhdPath
        Mount-VHD -Path $VhdPath

        Write-Output $Volume
        fsutil devdrv query $Drive
        Write-Output "Using Dev Drive at $Drive"
    } catch {
        Write-Warning "Failed to create Dev Drive, falling back to C:. $($_.Exception.Message)"
        $Drive = "C:"
    }
}

$Tmp = "$Drive\codex-tmp"
New-Item -Path $Tmp -ItemType Directory -Force | Out-Null

@(
    "DEV_DRIVE=$Drive"
    "TMP=$Tmp"
    "TEMP=$Tmp"
) | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
