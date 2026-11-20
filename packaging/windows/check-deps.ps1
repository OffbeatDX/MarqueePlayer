# Checks that a release folder runs on a Windows without the Visual C++ runtime installed:
# every DLL that the exe and the DLLs beside it import (normal and delay-loaded imports, read
# from the PE files themselves) is either in the folder or part of Windows itself (an API set,
# or a file in System32 that is Microsoft's and part of Windows, by its version resource). The Visual C++
# runtime (msvcp140, vcruntime140...) is never counted as Windows' even where it is installed.
# Windows looks in the application's own folder first for any DLL that is not a KnownDLL (the
# VC runtime is not one), so the copies beside the exe are the ones loaded.
#   powershell -NoProfile -ExecutionPolicy Bypass -File check-deps.ps1 -Folder <dir>
# Prints every import and where it comes from; exit code 1 when one would be missing.
param([Parameter(Mandatory = $true)][string]$Folder)
$ErrorActionPreference = 'Stop'

function Read-Imports([string]$Path) {
    $b = [IO.File]::ReadAllBytes($Path)
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    if ([BitConverter]::ToUInt32($b, $pe) -ne 0x00004550) { throw "$Path is not a PE file" }
    $sections = [BitConverter]::ToUInt16($b, $pe + 6)
    $optSize = [BitConverter]::ToUInt16($b, $pe + 20)
    $opt = $pe + 24
    $magic = [BitConverter]::ToUInt16($b, $opt)
    $dirs = if ($magic -eq 0x20B) { $opt + 112 } else { $opt + 96 }
    $secTable = $opt + $optSize
    $toOffset = {
        param([uint32]$rva)
        for ($i = 0; $i -lt $sections; $i++) {
            $s = $secTable + 40 * $i
            $va = [BitConverter]::ToUInt32($b, $s + 12)
            $size = [Math]::Max([BitConverter]::ToUInt32($b, $s + 8), [BitConverter]::ToUInt32($b, $s + 16))
            if ($rva -ge $va -and $rva -lt $va + $size) { return $rva - $va + [BitConverter]::ToUInt32($b, $s + 20) }
        }
        return -1
    }
    $name = {
        param([uint32]$rva)
        $o = & $toOffset $rva
        $e = $o
        while ($b[$e] -ne 0) { $e++ }
        [Text.Encoding]::ASCII.GetString($b, $o, $e - $o)
    }
    $out = @()
    # Imports: 20-byte descriptors, the name's RVA at +12, ending with an empty one.
    $rva = [BitConverter]::ToUInt32($b, $dirs + 8)
    if ($rva -ne 0) {
        $o = & $toOffset $rva
        while ([BitConverter]::ToUInt32($b, $o + 12) -ne 0) { $out += [pscustomobject]@{ Dll = (& $name ([BitConverter]::ToUInt32($b, $o + 12))); Delay = $false }; $o += 20 }
    }
    # Delay-load imports: 32-byte descriptors, the name's RVA at +4.
    $rva = [BitConverter]::ToUInt32($b, $dirs + 13 * 8)
    if ($rva -ne 0) {
        $o = & $toOffset $rva
        while ([BitConverter]::ToUInt32($b, $o + 4) -ne 0) { $out += [pscustomobject]@{ Dll = (& $name ([BitConverter]::ToUInt32($b, $o + 4))); Delay = $true }; $o += 32 }
    }
    $out
}

$Folder = (Resolve-Path -LiteralPath $Folder).Path
$local = @{}
Get-ChildItem -LiteralPath $Folder -File | Where-Object { $_.Extension -in '.exe', '.dll' } | ForEach-Object { $local[$_.Name.ToLowerInvariant()] = $_.FullName }
$system32 = Join-Path $env:SystemRoot 'System32'
$vcRuntime = '^(msvcp\d+.*|vcruntime\d+.*|concrt\d+|vccorlib\d+|vcomp\d+|msvcr\d+|vcamp\d+|mfc\d+.*)\.dll$'
$failed = 0
foreach ($file in $local.Values | Sort-Object) {
    Write-Host ("{0}" -f (Split-Path -Leaf $file))
    foreach ($imp in Read-Imports $file | Sort-Object Dll -Unique) {
        $dll = $imp.Dll.ToLowerInvariant()
        $kind = if ($imp.Delay) { 'delay-loaded' } else { 'imported' }
        if ($local.ContainsKey($dll)) { $from = 'in the folder' }
        elseif ($dll -match '^(api|ext)-ms-') { $from = 'Windows (API set)' }
        elseif ($dll -match $vcRuntime) { $from = 'MISSING: Visual C++ runtime, not beside the exe'; $failed++ }
        else {
            $sys = Join-Path $system32 $dll
            if (-not (Test-Path -LiteralPath $sys)) {
                if ($imp.Delay) { $from = 'not on this PC (delay-loaded: only if used)' }
                else { $from = 'MISSING: not in Windows'; $failed++ }
            } else {
                $info = (Get-Item -LiteralPath $sys).VersionInfo
                if ($info.CompanyName -match 'Microsoft' -and $info.ProductName -match 'Windows' -and $info.ProductName -notmatch 'Visual Studio') { $from = 'Windows' }
                else { $from = "MISSING: in System32 here, but from '$($info.CompanyName) $($info.ProductName)', not Windows"; $failed++ }
            }
        }
        Write-Host ("  {0,-44} {1,-13} {2}" -f $imp.Dll, $kind, $from)
    }
}
if ($failed -gt 0) {
    Write-Host "[X] $failed import(s) would be missing on a PC without them; see above."
    exit 1
}
Write-Host "[OK] Every import is in the folder or part of Windows."
exit 0
