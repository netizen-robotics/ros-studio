# Disposable CI runner only. Does not establish clean-machine or GUI acceptance.
param([Parameter(Mandatory=$true)][string]$PackageDir)
$ErrorActionPreference = 'Stop'
$package = (Resolve-Path $PackageDir).Path
$installer = Get-ChildItem "$package/*-setup.exe" | Select-Object -First 1
$root = Join-Path $env:RUNNER_TEMP 'ROS Studio installer smoke'
if (Test-Path $root) { throw 'Preserve previous installer evidence; choose a fresh runner' }
$evidence = "$PWD/artifacts/feasibility"
New-Item $evidence -ItemType Directory -Force | Out-Null
function Install-Suite([string]$Label) {
    $p = Start-Process $installer.FullName -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"/DIR=`"$root`"", "/LOG=`"$evidence/windows-setup-$Label.log`"") -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "Installer failed: $($p.ExitCode)" }
}
Install-Suite 'install'
$manifest = Get-Content "$package/payload.json" -Raw | ConvertFrom-Json
foreach ($property in $manifest.PSObject.Properties) {
    $actual = (Get-FileHash (Join-Path $root $property.Name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $property.Value) { throw "Installed payload mismatch: $($property.Name)" }
}
if (Test-Path "$root/studio-network-helper.exe") { throw 'Unexpected privileged helper' }
# Strip build-environment paths before executable linkage/configuration checks.
$savedPath = $env:PATH
$env:PATH = "$root/runtime/bin;$env:SystemRoot/System32;$env:SystemRoot"
try {
    foreach ($role in @('launcher','tools','visualizer')) {
        $p = Start-Process "$root/studio-$role.exe" -ArgumentList '--suite-configuration' -Wait -PassThru
        if ($p.ExitCode -ne 0) { throw "Installed $role configuration failed" }
    }
    $p = Start-Process "$root/studio-backend.exe" -ArgumentList '--package-linkage-check' -Wait -PassThru
    if ($p.ExitCode -ne 1) { throw "Backend linkage failed: $($p.ExitCode)" }
} finally { $env:PATH = $savedPath }
$data = Join-Path $env:LOCALAPPDATA 'ROS Studio'
New-Item $data -ItemType Directory -Force | Out-Null
Set-Content "$data/installer-ci-marker" 'preserve user data'
Install-Suite 'reinstall'
$p = Start-Process "$root/unins000.exe" -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"/LOG=`"$evidence/windows-setup-uninstall.log`"") -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "Uninstall failed: $($p.ExitCode)" }
if (Test-Path "$root/studio-launcher.exe") { throw 'Suite remains installed' }
if (!(Test-Path "$data/installer-ci-marker")) { throw 'User data marker was removed' }
Write-Output 'PASS: CI install, payload hashes, executable linkage, reinstall and uninstall. No native UI, clean-machine or ROS network acceptance.'
