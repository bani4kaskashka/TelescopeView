# Builds the Windows downloads for one architecture:
#   TelescopeView-<version>-windows-<arch>.zip        (portable: unzip anywhere and run)
#   TelescopeView-<version>-windows-<arch>-setup.exe  (installer, if Inno Setup 6 is installed)
#   TelescopeView-<version>-windows-<arch>.msix       (for the Microsoft Store, if the Windows SDK is installed)
# All carry a portable ffmpeg for videos (LGPL build from BtbN/FFmpeg-Builds, checked against
# its published checksums).
#
#   powershell -ExecutionPolicy Bypass -File packaging/build-windows.ps1 [-Arch x64|arm64]
#
# ARM64 needs the "MSVC ARM64 build tools" from the Visual Studio installer (for the C++ in
# CBR support); it builds on an x64 PC, but only runs on an ARM64 one.
param([ValidateSet("x64", "arm64")] [string]$Arch = "x64")
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
Set-Location (Split-Path $PSScriptRoot)

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value
$target = if ($Arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
$ffArch = if ($Arch -eq "arm64") { "winarm64" } else { "win64" }

rustup target add $target
cargo build --release --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$out = "target/windows"
$work = "$out/$Arch"
$pkg = "$work/TelescopeView"
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force "$pkg/ffmpeg/bin" | Out-Null

# ffmpeg, kept between runs.
$base = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest"
$zip = "ffmpeg-n9.0-latest-$ffArch-lgpl-shared-9.0.zip"
$cache = "target/ffmpeg-cache"
New-Item -ItemType Directory -Force $cache | Out-Null
Invoke-WebRequest "$base/checksums.sha256" -OutFile "$cache/checksums.sha256"
$want = ((Select-String -Path "$cache/checksums.sha256" -SimpleMatch $zip).Line -split '\s+')[0]
if (-not (Test-Path "$cache/$zip") -or (Get-FileHash "$cache/$zip" -Algorithm SHA256).Hash -ne $want) {
    Invoke-WebRequest "$base/$zip" -OutFile "$cache/$zip"
}
if ((Get-FileHash "$cache/$zip" -Algorithm SHA256).Hash -ne $want) { throw "$zip doesn't match its checksum" }
Expand-Archive "$cache/$zip" -DestinationPath "$work/ffmpeg-dl"
$src = (Get-ChildItem "$work/ffmpeg-dl" -Directory)[0].FullName

Copy-Item "target/$target/release/TelescopeView.exe" $pkg
Copy-Item "$src/bin/ffmpeg.exe", "$src/bin/ffprobe.exe", "$src/bin/*.dll" "$pkg/ffmpeg/bin"
Copy-Item "$src/LICENSE.txt" "$pkg/ffmpeg/LICENSE.txt"
Set-Content "$pkg/ffmpeg/README.txt" "FFmpeg ($zip, LGPL 2.1 or later), from https://github.com/BtbN/FFmpeg-Builds. Source code: https://ffmpeg.org/download.html"

# A clean Windows has no C++ runtime, and this PC or the CI machine does, so running it here
# wouldn't show the problem. Read what each file needs instead.
$dumpbin = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -find "VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe" | Select-Object -First 1
if (-not $dumpbin) { throw "dumpbin not found" }
Get-ChildItem $pkg -Recurse -Include *.exe, *.dll | ForEach-Object {
    if ((& $dumpbin -nologo -dependents $_.FullName) -match '(?i)^\s*(msvc[pr]|vcruntime)\S*\.dll') { throw "$($_.Name) needs the C++ runtime" }
}

$name = "TelescopeView-$version-windows-$Arch"
Compress-Archive -Path $pkg -DestinationPath "$out/$name.zip" -Force
Write-Host "Built $out/$name.zip"

# The Microsoft Store package.
& "$PSScriptRoot/windows/build-msix.ps1" -Source (Resolve-Path $pkg).Path -Version $version -Arch $Arch -OutDir (Resolve-Path $out).Path

# The installer.
$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue).Source,
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) {
    Write-Host "(no installer: install Inno Setup 6, e.g. winget install JRSoftware.InnoSetup)"
    return
}
$full = (Resolve-Path $out).Path
& $iscc /Q "/DVersion=$version" "/DArch=$Arch" "/DSource=$((Resolve-Path $pkg).Path)" "/DOutDir=$full" packaging/windows/TelescopeView.iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
Remove-Item -Recurse -Force $work
Write-Host "Built $out/$name-setup.exe"
