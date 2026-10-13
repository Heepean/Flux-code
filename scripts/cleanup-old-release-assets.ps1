[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'Medium')]
param(
    [string]$CurrentReleaseTag
)

$ErrorActionPreference = 'Stop'

$repository = $env:GITHUB_REPOSITORY
$token = $env:GITHUB_TOKEN
if (-not $repository) {
    throw 'GITHUB_REPOSITORY is required.'
}
if (-not $token -and -not $WhatIfPreference) {
    throw 'GITHUB_TOKEN is required unless -WhatIf is used.'
}

$headers = @{
    Accept = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
    'User-Agent' = 'Flux-Hands-Release-Cleanup'
}
if ($token) {
    $headers.Authorization = "Bearer $token"
}

$base = "https://api.github.com/repos/$repository"
$currentTag = if ($CurrentReleaseTag) { $CurrentReleaseTag } else { $env:GITHUB_REF_NAME }
if ($currentTag -notmatch '^v\d+\.\d+\.\d+(-beta\.\d+)?$') {
    Write-Host "Skipping release-asset cleanup for non-release ref '$currentTag'. Cleanup runs only after a versioned release is created."
    exit 0
}
$current = Invoke-RestMethod -Method Get -Uri "$base/releases/tags/$currentTag" -Headers $headers
if ($current.draft) {
    throw "Refusing cleanup because current release '$currentTag' is still a draft."
}
$latest = $null
try {
    $latest = Invoke-RestMethod -Method Get -Uri "$base/releases/latest" -Headers $headers
}
catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 404) { throw }
    Write-Host 'No stable release exists yet; retaining the current published prerelease.'
}
$releases = @()
$page = 1
do {
    $batch = Invoke-RestMethod -Method Get -Uri "$base/releases?per_page=100&page=$page" -Headers $headers
    $releases += $batch
    $page++
} while ($batch.Count -eq 100)
$keepIds = @($current.id)
if ($latest -and $latest.id) { $keepIds += $latest.id }
$staleReleases = @($releases | Where-Object { $_.id -notin $keepIds })
$removed = 0

foreach ($release in $staleReleases) {
    foreach ($asset in $release.assets) {
        if ($asset.name -notmatch '(?i)(setup\.exe|\.msi|\.apk$|\.exe\.sig$|^latest\.json$)') {
            continue
        }

        $target = "$($release.tag_name)/$($asset.name)"
        if ($PSCmdlet.ShouldProcess($target, 'Delete stale GitHub release asset')) {
            Invoke-RestMethod -Method Delete -Uri "$base/releases/assets/$($asset.id)" -Headers $headers | Out-Null
            Write-Host "Removed stale release asset $target"
            $removed++
        }
    }
}

$latestTag = if ($latest) { $latest.tag_name } else { 'none' }
Write-Host "Kept current release $($current.tag_name) and latest stable release $latestTag; removed $removed installer/updater assets from older releases."
