param(
    [Parameter(Mandatory=$true)][string]$ListFile,
    [string]$EmbeddedSignTool
)

$ErrorActionPreference = "Stop"
if ($PSVersionTable.PSVersion.Major -le 5 -and $env:SystemRoot) {
    $env:PSModulePath = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\Modules"
}
$paths = Get-Content -LiteralPath $ListFile -Raw -Encoding UTF8 | ConvertFrom-Json
$results = @(foreach ($path in $paths) {
    $signature = Get-AuthenticodeSignature -LiteralPath $path
    $versionInfo = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($path)
    $signer = $signature.SignerCertificate
    $status = $signature.Status.ToString()
    if ($EmbeddedSignTool -and $signature.SignatureType -eq 'Catalog') {
        # Get-AuthenticodeSignature prefers an installed system catalog over the
        # signature that travels with the DLL. Verify all embedded signatures
        # before obtaining the embedded certificate for the existing allowlist.
        & $EmbeddedSignTool verify /pa /all $path | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Embedded Authenticode verification failed: $path" }
        $signer = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
            [System.Security.Cryptography.X509Certificates.X509Certificate]::CreateFromSignedFile($path)
        )
        $status = 'Valid'
    }
    $signerName = if ($signer) {
        $signer.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    }
    @{
        status = $status
        signerName = $signerName
        signerSubject = $signer.Subject
        thumbprint = $signer.Thumbprint
        timestamped = ($null -ne $signature.TimeStamperCertificate)
        productVersion = $versionInfo.ProductVersion
        productName = $versionInfo.ProductName
    }
})
ConvertTo-Json -InputObject $results -Compress
