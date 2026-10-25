$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target/release/clipvault.exe dist/
Copy-Item README.md,LICENSE dist/
Compress-Archive -Path dist/clipvault.exe,dist/README.md,dist/LICENSE -DestinationPath dist/ClipVault-Windows-portable.zip -Force
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
if (!(Test-Path $iscc)) { choco install innosetup --no-progress -y; if ($LASTEXITCODE -ne 0) { throw 'Inno Setup installation failed' } }
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
& $iscc "/DAppVer=$version" packaging/windows.iss
if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' }
Remove-Item dist/clipvault.exe,dist/README.md,dist/LICENSE
