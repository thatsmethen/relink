# Build/validate an unsigned package only. Does not install, sign, or change policy.
[CmdletBinding()]
param(
    [string]$CacheDirectory = (Join-Path $env:TEMP 'relink-wdk-10.0.26100.6584'),
    [string]$OutputDirectory,
    [switch]$SkipCatalog
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$Pin = Get-Content (Join-Path $PSScriptRoot 'toolchain.json') -Raw | ConvertFrom-Json
if (!$OutputDirectory) { $OutputDirectory = $env:RELINK_OUTPUT }
if (!$OutputDirectory) { $OutputDirectory = $env:RAVELINK_OUTPUT }
if (!$OutputDirectory) { $OutputDirectory = Join-Path $Repo 'dist/windows-driver' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force $CacheDirectory, $OutputDirectory | Out-Null
# Remove only generated package files; never let a failed rerun expose an old
# catalog or success report alongside a newly built binary.
foreach ($generated in @('RaveLink.sys', 'RaveLink.inf', 'RaveLink.cat', 'ravelink_windows_driver.pdb', 'ReLink.sys', 'ReLink.inf', 'ReLink.cat', 'relink_windows_driver.pdb', 'pe-evidence.json', 'build-evidence.json', 'DRIVER-RUNTIME-NOTICES.txt', 'runtime-notices')) {
    $path = Join-Path $OutputDirectory $generated
    if (Test-Path $path) { Remove-Item -Recurse -Force $path }
}

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
function Get-PinnedPackage($Package) {
    $archive = Join-Path $CacheDirectory "$($Package.id).$($Package.version).nupkg"
    if (!(Test-Path $archive)) {
        $url = "https://api.nuget.org/v3-flatcontainer/$($Package.id)/$($Package.version)/$($Package.id).$($Package.version).nupkg"
        Invoke-WebRequest -Uri $url -OutFile "$archive.download"
        Move-Item "$archive.download" $archive -Force
    }
    $hash = [Security.Cryptography.SHA512]::Create()
    $stream = [IO.File]::OpenRead($archive)
    try { $actual = [Convert]::ToBase64String($hash.ComputeHash($stream)) }
    finally { $stream.Dispose(); $hash.Dispose() }
    if ($actual -ne $Package.sha512_base64) { throw "Package checksum mismatch: $archive" }
    $destination = Join-Path $CacheDirectory "$($Package.id).$($Package.version)"
    # Re-extract verified archives so stale extracted files cannot bypass validation.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    if (Test-Path $destination) { Remove-Item -Recurse -Force $destination }
    [IO.Compression.ZipFile]::ExtractToDirectory($archive, $destination)
    return $destination
}

$Wdk = Get-PinnedPackage $Pin.packages[0]
$Sdk = Get-PinnedPackage $Pin.packages[1]
$Version = $Pin.sdk_header_version
$Manifest = Join-Path $Repo 'crates/relink-windows-driver/Cargo.toml'
$BuildDirectory = Join-Path $Repo 'target/windows-driver'
$oldWdk = $env:RAVELINK_WDK_LIB
$oldTarget = $env:CARGO_TARGET_DIR
$oldFlags = $env:CARGO_ENCODED_RUSTFLAGS
$oldRustFlags = $env:RUSTFLAGS
try {
    # Toolchain installation is deliberately left to the CI/developer setup.
    $compiler = (& rustc "+$($Pin.rust)" --version) -join ' '
    if ($LASTEXITCODE -ne 0 -or !$compiler.Contains($Pin.rust_commit)) { throw 'Install the exact toolchain in toolchain.json with rust-src and the Windows target.' }
    $sysroot = (& rustc "+$($Pin.rust)" --print sysroot).Trim()
    $hostTriple = ((& rustc "+$($Pin.rust)" -vV | Select-String '^host: ') -replace '^host: ', '').Trim()
    $linker = Join-Path $sysroot "lib/rustlib/$hostTriple/bin/rust-lld.exe"
    if (!(Test-Path $linker)) { throw "Missing Rust linker: $linker" }
    $env:RAVELINK_WDK_LIB = Join-Path $Wdk "c/Lib/$Version/km/x64"
    $env:CARGO_TARGET_DIR = $BuildDirectory
    $env:RUSTFLAGS = $null
    $env:CARGO_ENCODED_RUSTFLAGS = @('-C', "linker=$linker", '-C', 'no-redzone=yes', '-C', 'no-vectorize-loops', '-C', 'no-vectorize-slp') -join [char]31
    Invoke-Checked cargo @("+$($Pin.rust)", 'build', '-Zbuild-std=core', '--locked', '--manifest-path', $Manifest, '--target', $Pin.target, '--release', '--features', 'kernel')
    # Host tests use mocks and the native host ABI, never Windows driver loading.
    $env:CARGO_ENCODED_RUSTFLAGS = $null
    Invoke-Checked cargo @("+$($Pin.rust)", 'test', '--locked', '--manifest-path', $Manifest)

    # SDK ABI declarations are independently checked with the v143 14.44 compiler.
    # This object is compile-only and is never linked into ReLink.sys.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs = (& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
    if (!$vs) { throw 'Visual Studio C++ tools are required for SDK ABI validation.' }
    $toolset = Get-ChildItem (Join-Path $vs 'VC/Tools/MSVC') -Directory | Where-Object Name -Like "$($Pin.msvc_validation_toolset).*" | Sort-Object Name -Descending | Select-Object -First 1
    if (!$toolset) { throw "MSVC $($Pin.msvc_validation_toolset) is required for the compile-only ABI check." }
    $cl = Join-Path $toolset.FullName 'bin/Hostx64/x64/cl.exe'
    $includes = @((Join-Path $Wdk "c/Include/$Version/km"), (Join-Path $Wdk "c/Include/$Version/km/crt"), (Join-Path $Sdk "c/Include/$Version/shared"), (Join-Path $Sdk "c/Include/$Version/ucrt"), (Join-Path $toolset.FullName 'include'))
    $arguments = @('/nologo', '/c', '/std:c++17', '/D_AMD64_=1', '/D_WIN64', '/D_WIN32_WINNT=0x0a00', '/DNTDDI_VERSION=0x0A00000C')
    foreach ($include in $includes) { $arguments += "/I$include" }
    $arguments += @("/Fo$(Join-Path $BuildDirectory 'abi-check.obj')", (Join-Path $PSScriptRoot 'tests/abi-check.cpp'))
    Invoke-Checked $cl $arguments

    $binary = Join-Path $BuildDirectory "$($Pin.target)/release/relink_windows_driver.dll"
    $driver = Join-Path $OutputDirectory 'ReLink.sys'
    Copy-Item $binary $driver -Force
    Copy-Item (Join-Path $PSScriptRoot 'ReLink.inf') $OutputDirectory -Force
    Copy-Item (Join-Path $Repo 'LICENSE') $OutputDirectory -Force
    Copy-Item (Join-Path $PSScriptRoot 'PROVENANCE.md') $OutputDirectory -Force
    Copy-Item (Join-Path $PSScriptRoot 'toolchain.json') $OutputDirectory -Force
    Invoke-Checked python @((Join-Path $PSScriptRoot 'runtime-notices.py'), '--sysroot', $sysroot, '--output', $OutputDirectory)
    Copy-Item (Join-Path $BuildDirectory "$($Pin.target)/release/relink_windows_driver.pdb") $OutputDirectory -Force
    Invoke-Checked python @((Join-Path $PSScriptRoot 'tests/check-pe.py'), $driver, '--output', (Join-Path $OutputDirectory 'pe-evidence.json'))
    $infverif = Join-Path $Wdk "c/tools/$Version/x64/infverif.exe"
    Invoke-Checked $infverif @('/w', (Join-Path $OutputDirectory 'ReLink.inf'))
    if (!$SkipCatalog) {
        $inf2cat = Join-Path $Wdk "c/bin/$Version/x86/Inf2Cat.exe"
        Invoke-Checked $inf2cat @("/driver:$OutputDirectory", '/os:10_VB_X64', '/uselocaltime')
    }
    @{
        rust = $compiler; target = $Pin.target; wdk = $Pin.packages[0].version
        sdk = $Pin.packages[1].version; abi_compiler = $toolset.Name
        kernel_loaded = $false; signed = $false; catalog_created = !$SkipCatalog
    } | ConvertTo-Json | Set-Content (Join-Path $OutputDirectory 'build-evidence.json')
    Write-Host "Unsigned package and validation evidence: $OutputDirectory"
} finally {
    $env:RAVELINK_WDK_LIB = $oldWdk
    $env:CARGO_TARGET_DIR = $oldTarget
    $env:CARGO_ENCODED_RUSTFLAGS = $oldFlags
    $env:RUSTFLAGS = $oldRustFlags
}
