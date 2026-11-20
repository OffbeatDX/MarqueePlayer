<#
.SYNOPSIS
  The headless test emulator for Marquee Player: its own AVD (marquee-test) on port 5580.

.DESCRIPTION
  start             Creates the AVD if it is missing, boots it headless and waits for the boot.
                    -Gpu <mode> passes -gpu (swiftshader_indirect if qemu crashes on the host GPU).
  stop              Stops this emulator only (emulator-5580).
  push-test-config  Copies ~/.marquee-android/test-server.properties into the debug app's private
                    files (through run-as, so it never sits in shared storage) without printing it.
                    The debug build reads it at start, signs in with it and forces read-only mode.
                    -Benchmark pushes it to the benchmark build (.bench) instead: that build is not
                    debuggable, so run-as can't reach it; adbd restarts as root (this emulator only)
                    and the file is written straight into the app's folder, owned by the app.

  Every adb call names -s emulator-5580; no other device is touched.

  Audio: -no-audio is enough for playback tests. The guest still has its audio HAL, so ExoPlayer
  opens an AudioTrack and plays (media_session shows PLAYING with the position advancing,
  `dumpsys media.audio_flinger` shows the track and its volume); nothing reaches the host, so the
  PC stays silent.
#>
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet('start', 'stop', 'push-test-config')]
    [string]$Command,
    [switch]$Benchmark,
    # start: the emulator's GPU mode, e.g. swiftshader_indirect (software) when the host GPU path
    # crashes qemu; the emulator's own default when not given.
    [string]$Gpu
)

$ErrorActionPreference = 'Stop'

$Avd = 'marquee-test'
$Port = 5580
$Serial = "emulator-$Port"
$Image = 'system-images;android-35;google_apis;x86_64'
$DebugPackage = 'com.offbeat.marquee.player.debug'
$BenchPackage = 'com.offbeat.marquee.player.bench'

$Sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
$Adb = Join-Path $Sdk 'platform-tools\adb.exe'
$Emulator = Join-Path $Sdk 'emulator\emulator.exe'
$AvdManager = Join-Path $Sdk 'cmdline-tools\latest\bin\avdmanager.bat'
if (-not $env:JAVA_HOME) {
    $jdk = Join-Path $env:USERPROFILE '.jdks\temurin-17'
    if (Test-Path $jdk) { $env:JAVA_HOME = $jdk }
}

function Test-Booted {
    $out = & $Adb -s $Serial shell getprop sys.boot_completed 2>$null
    return ($LASTEXITCODE -eq 0 -and "$out".Trim() -eq '1')
}

function Start-TestEmulator {
    $avdList = & $Emulator -list-avds
    if ($avdList -notcontains $Avd) {
        Write-Host "Creating AVD $Avd from $Image"
        'no' | & $AvdManager create avd -n $Avd -k $Image -d pixel_6 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "avdmanager failed ($LASTEXITCODE)" }
    }
    if (Test-Booted) { Write-Host "$Serial is already running"; return }

    Write-Host "Booting $Avd headless on port $Port"
    $emuArgs = @('-avd', $Avd, '-port', "$Port", '-no-window', '-no-audio', '-no-boot-anim', '-no-snapshot-save')
    if ($Gpu) { $emuArgs += @('-gpu', $Gpu) }
    Start-Process -FilePath $Emulator -WindowStyle Hidden -ArgumentList $emuArgs | Out-Null

    $deadline = (Get-Date).AddMinutes(5)
    while (-not (Test-Booted)) {
        if ((Get-Date) -gt $deadline) { throw "$Serial did not boot within 5 minutes" }
        Start-Sleep -Seconds 3
    }
    # Animations off: steadier screenshots and faster UI checks.
    & $Adb -s $Serial shell settings put global window_animation_scale 0 | Out-Null
    & $Adb -s $Serial shell settings put global transition_animation_scale 0 | Out-Null
    & $Adb -s $Serial shell settings put global animator_duration_scale 0 | Out-Null
    Write-Host "$Serial booted"
}

function Stop-TestEmulator {
    & $Adb -s $Serial emu kill 2>$null | Out-Null
    Write-Host "Stopped $Serial"
}

function Push-BenchConfig {
    $config = Join-Path $env:USERPROFILE '.marquee-android\test-server.properties'
    if (-not (Test-Path $config)) { throw "No test config at ~/.marquee-android/test-server.properties" }
    & $Adb -s $Serial shell pm path $BenchPackage 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "$BenchPackage is not installed on $Serial" }
    & $Adb -s $Serial root | Out-Null
    & $Adb -s $Serial wait-for-device
    $dir = "/data/data/$BenchPackage/files"
    $owner = "$(& $Adb -s $Serial shell stat -c %u:%g /data/data/$BenchPackage)".Trim()
    & $Adb -s $Serial shell "mkdir -p $dir && chown $owner $dir && chmod 700 $dir" | Out-Null
    $remote = "sh -c 'cat > $dir/test-server.properties'"
    $p = Start-Process -FilePath $Adb -NoNewWindow -Wait -PassThru `
        -ArgumentList @('-s', $Serial, 'exec-in', "`"$remote`"") -RedirectStandardInput $config
    if ($p.ExitCode -ne 0) { throw "Pushing the test config failed ($($p.ExitCode))" }
    & $Adb -s $Serial shell "chown $owner $dir/test-server.properties && chmod 600 $dir/test-server.properties && restorecon -R $dir 2>/dev/null" 2>&1 | Out-Null
    $size = & $Adb -s $Serial shell stat -c %s "$dir/test-server.properties"
    if ("$size".Trim() -ne "$((Get-Item $config).Length)") { throw "The test config did not arrive whole" }
    & $Adb -s $Serial unroot | Out-Null
    Write-Host "Test config pushed to $BenchPackage (restart the app to use it)"
}

function Push-TestConfig {
    $config = Join-Path $env:USERPROFILE '.marquee-android\test-server.properties'
    if (-not (Test-Path $config)) { throw "No test config at ~/.marquee-android/test-server.properties" }
    & $Adb -s $Serial shell pm path $DebugPackage 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "$DebugPackage is not installed on $Serial" }

    # The file goes through stdin straight into the app's sandbox; it is never echoed.
    $remote = "run-as $DebugPackage sh -c 'mkdir -p files && cat > files/test-server.properties'"
    $expected = "$((Get-Item $config).Length)"
    # Right after an install the first attempt can land short; check and retry a few times.
    for ($attempt = 1; ; $attempt++) {
        $p = Start-Process -FilePath $Adb -NoNewWindow -Wait -PassThru `
            -ArgumentList @('-s', $Serial, 'exec-in', "`"$remote`"") `
            -RedirectStandardInput $config
        if ($p.ExitCode -ne 0) { throw "Pushing the test config failed ($($p.ExitCode))" }
        # Check it landed without showing it: compare sizes only.
        $size = & $Adb -s $Serial shell "run-as $DebugPackage stat -c %s files/test-server.properties" 2>$null
        if ("$size".Trim() -eq $expected) { break }
        if ($attempt -ge 3) { throw "The test config did not arrive whole" }
        Start-Sleep -Seconds 2
    }
    # run-as leaves it world-readable (0666); make it the app's only. A separate call: exec-in ends
    # the remote shell as soon as stdin closes, so a chmod chained after cat may never run.
    & $Adb -s $Serial shell "run-as $DebugPackage chmod 600 files/test-server.properties" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Could not make the test config private" }
    Write-Host "Test config pushed to $DebugPackage (restart the app to use it)"
}

switch ($Command) {
    'start' { Start-TestEmulator }
    'stop' { Stop-TestEmulator }
    'push-test-config' { if ($Benchmark) { Push-BenchConfig } else { Push-TestConfig } }
}
