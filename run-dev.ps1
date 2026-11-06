$ErrorActionPreference = 'Stop'

$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$vsDevCmd = 'C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat'
if (-not (Test-Path $vsDevCmd)) {
    Write-Error 'No se encuentra VsDevCmd.bat. Instala Visual Studio 2022 con Desktop development with C++.'
    exit 1
}

# Import the MSVC and Windows SDK environment into this PowerShell process for Cargo.
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
$kernelLib = if ($sdkLib -and (Test-Path $sdkLib)) { Get-ChildItem $sdkLib -Filter kernel32.lib -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1 }
if (-not $kernelLib) {
    Write-Error 'Falta el Windows SDK (kernel32.lib). En Visual Studio Installer añade Windows 10/11 SDK y vuelve a ejecutar este script.'
    exit 1
}

npm run tauri:dev
exit $LASTEXITCODE
