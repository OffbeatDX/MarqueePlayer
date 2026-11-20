<#
.SYNOPSIS
  A Windows convenience around Gradle for Marquee Player for Android.

.DESCRIPTION
  Gradle is the build: on any system, ./gradlew :app:assembleDebug, ./gradlew check and
  ./gradlew :app:assembleRelease do everything (README, "Building"). This script only adds what
  Gradle can't do on a fresh Windows checkout, then runs the same Gradle tasks:
  - sets JAVA_HOME to the JDK 17 in ~/.jdks when it is not set, and writes local.properties
    (sdk.dir) when it is missing;
  - on the first -Release, creates the release key: ~/.marquee-android/release.jks, with its alias
    and password in ~/.marquee-android/keystore.properties (keytool, RSA 4096, 30 years, a random
    password that is never printed). Back that folder up: an app signed with a lost key can only
    be updated by uninstalling it first. Skipped when MARQUEE_KEYSTORE_FILE is set (the key then
    comes from the environment, see app/build.gradle.kts);
  - stops when a release came out unsigned, and runs adb install with -Install.
  Works from any directory. The APKs stay where Gradle puts them:
  app/build/outputs/apk/<variant>/marquee-player-<version>-<variant>.apk.

.EXAMPLE
  ./build.ps1                          # debug APK (gradlew :app:assembleDebug)
  ./build.ps1 -Test                    # unit tests of :core and :app, and :core's API scan
  ./build.ps1 -Release                 # signed release APK (gradlew :app:assembleRelease)
  ./build.ps1 -Install emulator-5580   # debug APK, installed on that device
  ./build.ps1 -Release -Install <serial>          # the release, installed on that device
  ./build.ps1 -Benchmark -Install emulator-5580   # release-like, profileable, read-only (.bench)
#>
param(
    [switch]$Release,
    [switch]$Test,
    [switch]$Benchmark,
    [string]$Install
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

# ---- Environment -----------------------------------------------------------------
$jdk = Join-Path $env:USERPROFILE '.jdks\temurin-17'
if (-not $env:JAVA_HOME -or -not (Test-Path (Join-Path $env:JAVA_HOME 'bin\java.exe'))) {
    if (-not (Test-Path $jdk)) { throw "JDK 17 not found; set JAVA_HOME" }
    $env:JAVA_HOME = $jdk
}
$sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
$localProps = Join-Path $root 'local.properties'
if (-not (Test-Path $localProps)) {
    if (-not (Test-Path $sdk)) { throw "Android SDK not found; set ANDROID_HOME" }
    # local.properties is read as a Java properties file: backslashes and the colon are escaped.
    $escaped = $sdk.Replace('\', '\\').Replace(':', '\:')
    Set-Content -Path $localProps -Value "sdk.dir=$escaped" -Encoding ascii
}

# ---- The release key (first run) ----------------------------------------------------
function Initialize-ReleaseKey {
    $dir = Join-Path $env:USERPROFILE '.marquee-android'
    $store = Join-Path $dir 'release.jks'
    $props = Join-Path $dir 'keystore.properties'
    if ((Test-Path $store) -and (Test-Path $props)) { return }
    if ((Test-Path $store) -or (Test-Path $props)) {
        throw "Only one of release.jks and keystore.properties is in $dir; restore the other from your backup (a new key can't update the installed app)"
    }
    New-Item -ItemType Directory -Force $dir | Out-Null
    # A random password (192 bits, hex), handed to keytool through the environment, never printed.
    $bytes = New-Object byte[] 24
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
    $password = -join ($bytes | ForEach-Object { $_.ToString('x2') })
    $keytool = Join-Path $env:JAVA_HOME 'bin\keytool.exe'
    $env:MP_KEYSTORE_PASSWORD = $password
    try {
        & $keytool -genkeypair -keystore $store -storetype PKCS12 -alias marquee `
            -keyalg RSA -keysize 4096 -validity 10950 -dname 'CN=Marquee Player' `
            -storepass:env MP_KEYSTORE_PASSWORD -keypass:env MP_KEYSTORE_PASSWORD 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "keytool failed ($LASTEXITCODE)" }
    } finally {
        Remove-Item Env:\MP_KEYSTORE_PASSWORD -ErrorAction SilentlyContinue
    }
    # PKCS12 keeps one password for the store and the key.
    @(
        '# The Marquee Player release key. Keep this file and release.jks together, and back them up.',
        'storeFile=release.jks',
        'keyAlias=marquee',
        "storePassword=$password",
        "keyPassword=$password"
    ) | Set-Content -Path $props -Encoding ascii
    Write-Host "Created the release key in $dir (release.jks, keystore.properties). Back that folder up."
}
if ($Release -and -not $env:MARQUEE_KEYSTORE_FILE) { Initialize-ReleaseKey }

# ---- Gradle: the same tasks as on any other system (README, "Building") -----------
$variant = if ($Release) { 'Release' } elseif ($Benchmark) { 'Benchmark' } else { 'Debug' }
$tasks = @()
if ($Test) { $tasks += ':core:check', ":app:test${variant}UnitTest" }
$assemble = (-not $Test) -or $Release -or $Benchmark -or $Install
if ($assemble) { $tasks += ":app:assemble$variant" }

$started = Get-Date
& (Join-Path $root 'gradlew.bat') -p $root @tasks --console=plain
if ($LASTEXITCODE -ne 0) { throw "Gradle failed ($LASTEXITCODE)" }
Write-Host ("Gradle took {0:n0} s" -f ((Get-Date) - $started).TotalSeconds)
if (-not $assemble) { return }

# ---- The APK, and adb install ------------------------------------------------------
$outDir = Join-Path $root "app\build\outputs\apk\$($variant.ToLower())"
$apk = Get-ChildItem $outDir -Filter '*.apk' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $apk) { throw "No APK in $outDir" }
if ($Release -and $apk.Name -like '*unsigned*') { throw "The release APK is unsigned; is ~/.marquee-android/keystore.properties readable?" }
Write-Host "APK: $($apk.FullName)"

if ($Install) {
    & (Join-Path $sdk 'platform-tools\adb.exe') -s $Install install -r $apk.FullName
    if ($LASTEXITCODE -ne 0) { throw "adb install failed ($LASTEXITCODE)" }
    Write-Host "Installed on $Install"
}
