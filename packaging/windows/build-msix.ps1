# Packs a built TelescopeView folder (the .exe and ffmpeg\) into the .msix for the Microsoft Store,
# which signs it. Run by packaging/build-windows.ps1; needs the Windows SDK. The package won't
# install before it's signed, so it's not for the GitHub release.
param(
    [Parameter(Mandatory)] [string]$Source,
    [Parameter(Mandatory)] [string]$Version,
    [Parameter(Mandatory)] [ValidateSet("x64", "arm64")] [string]$Arch,
    [Parameter(Mandatory)] [string]$OutDir,
    # From Partner Center: Apps and games > TelescopeView > Product identity. The Store refuses any others.
    [string]$Name = "TelescopeView.Test",
    [string]$Publisher = "CN=TelescopeView-Test",
    [string]$PublisherName = "TelescopeView Test"
)
$ErrorActionPreference = "Stop"

$kit = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\makeappx.exe" -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Directory.Parent.Name } | Select-Object -Last 1
if (-not $kit) {
    Write-Host "(no .msix: install the Windows SDK)"
    return
}
$makeappx = $kit.FullName
$makepri = Join-Path $kit.DirectoryName "makepri.exe"

$stage = Join-Path ([IO.Path]::GetTempPath()) "TelescopeView-msix-$PID"
New-Item -ItemType Directory -Force $stage | Out-Null
try {
    Copy-Item "$Source\*" $stage -Recurse
    Copy-Item "$PSScriptRoot\msix-assets" "$stage\Assets" -Recurse
    $manifest = (Get-Content "$PSScriptRoot\AppxManifest.xml" -Raw -Encoding UTF8).
        Replace('$Name$', $Name).Replace('$Publisher$', $Publisher).Replace('$PublisherName$', $PublisherName).
        Replace('$Version$', "$Version.0").Replace('$Arch$', $Arch)
    [IO.File]::WriteAllText("$stage\AppxManifest.xml", $manifest)

    # Lets Windows pick the right icon size; without it only the plain file names are found.
    $config = Join-Path ([IO.Path]::GetTempPath()) "TelescopeView-pri-$PID.xml"
    & $makepri createconfig /cf $config /dq en-US /o | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "makepri createconfig failed" }
    & $makepri new /pr $stage /cf $config /of "$stage\resources.pri" /o | Out-Null
    $priExit = $LASTEXITCODE
    Remove-Item $config
    if ($priExit -ne 0) { throw "makepri failed" }

    New-Item -ItemType Directory -Force $OutDir | Out-Null
    $msix = Join-Path $OutDir "TelescopeView-$Version-windows-$Arch.msix"
    & $makeappx pack /o /d $stage /p $msix | Out-Null
    if ($LASTEXITCODE -ne 0) {
        # Says what's wrong with the manifest.
        & $makeappx pack /o /v /d $stage /p $msix
        throw "makeappx failed"
    }
    Write-Host "Built $msix"
    if ($Name -eq "TelescopeView.Test") { Write-Host "(test identity: the Store won't take it until the values at the top of build-msix.ps1 are filled in)" }
} finally {
    Remove-Item -Recurse -Force $stage
}
