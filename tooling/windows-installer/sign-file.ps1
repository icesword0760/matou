param(
    [Parameter(Mandatory=$true)][string]$SignTool,
    [Parameter(Mandatory=$true)][string]$Thumbprint,
    [Parameter(Mandatory=$true)][string]$Timestamp,
    [Parameter(Mandatory=$true)][string]$UninstallerDir,
    [Parameter(Mandatory=$true)][string]$Target
)

$ErrorActionPreference = "Stop"
& $SignTool sign /sha1 $Thumbprint /fd SHA256 /tr $Timestamp /td SHA256 $Target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Inno embeds this signed file, then removes its temporary input.
$tempUninstaller = Join-Path $UninstallerDir "uninst.e64.tmp"
if ([System.IO.Path]::GetFullPath($Target) -ieq $tempUninstaller) {
    Copy-Item -LiteralPath $Target -Destination (Join-Path $UninstallerDir "uninstaller.exe")
}
