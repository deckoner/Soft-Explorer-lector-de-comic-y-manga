$ErrorActionPreference = 'Stop'

# Carga el toolchain MSVC/SDK que necesita el target Windows de Rust.
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$vsDevCmd = 'C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat'
if (-not (Test-Path $vsDevCmd)) {
    Write-Error 'No se encuentra Visual Studio 2022 Community. Instala Desktop development with C++.'
    exit 1
}

$environmentLines = & $env:ComSpec /d /s /c "`"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul && set"
foreach ($line in $environmentLines) {
    $separator = $line.IndexOf('=')
    if ($separator -le 0) { continue }
    $name = $line.Substring(0, $separator)
    if ($name -in @('PATH', 'INCLUDE', 'LIB', 'LIBPATH', 'WindowsSdkDir', 'WindowsSDKVersion', 'UniversalCRTSdkDir')) {
        Set-Item -Path "Env:$name" -Value $line.Substring($separator + 1)
    }
}
$env:PATH = "$cargoBin;$env:PATH"

$sdkLib = if ($env:WindowsSdkDir) { Join-Path $env:WindowsSdkDir 'Lib' } else { $null }
$x64KernelLib = if ($sdkLib -and (Test-Path $sdkLib)) {
    Get-ChildItem $sdkLib -Filter kernel32.lib -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '[\\/]um[\\/]x64[\\/]kernel32\.lib$' } |
        Select-Object -First 1
}
if (-not $x64KernelLib) {
    Write-Error 'Falta el Windows SDK x64. Añade Windows 10/11 SDK en Visual Studio Installer.'
    exit 1
}

npm run tauri:build
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$bundlePath = Join-Path $PSScriptRoot 'src-tauri\target\release\bundle'
Write-Host "`nInstaladores generados en: $bundlePath" -ForegroundColor Green
Get-ChildItem $bundlePath -Recurse -File -Include *.msi,*.exe -ErrorAction SilentlyContinue |
    Select-Object -ExpandProperty FullName
