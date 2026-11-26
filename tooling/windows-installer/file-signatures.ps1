param([Parameter(Mandatory=$true)][string]$ListFile)

$ErrorActionPreference = "Stop"
$paths = Get-Content -LiteralPath $ListFile -Raw -Encoding UTF8 | ConvertFrom-Json
$results = @(foreach ($path in $paths) {
    $signature = Get-AuthenticodeSignature -LiteralPath $path
    $versionInfo = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($path)
    $signer = $signature.SignerCertificate
    $signerName = if ($signer) {
        $signer.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    }
    @{
        status = $signature.Status.ToString()
        signerName = $signerName
        signerSubject = $signer.Subject
        thumbprint = $signer.Thumbprint
        timestamped = ($null -ne $signature.TimeStamperCertificate)
        productVersion = $versionInfo.ProductVersion
        productName = $versionInfo.ProductName
    }
})
ConvertTo-Json -InputObject $results -Compress
