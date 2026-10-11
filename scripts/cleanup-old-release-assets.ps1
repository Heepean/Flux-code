$ErrorActionPreference = 'Stop'

$repository = $env:GITHUB_REPOSITORY
$token = $env:GITHUB_TOKEN
if (-not $repository -or -not $token) {
    throw 'GITHUB_REPOSITORY and GITHUB_TOKEN are required.'
}

$headers = @{
    Authorization = "Bearer $token"
    Accept = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
    'User-Agent' = 'Flux-Hands-Release-Cleanup'
}

$base = "https://api.github.com/repos/$repository"
$latest = Invoke-RestMethod -Method Get -Uri "$base/releases/latest" -Headers $headers
$releases = Invoke-RestMethod -Method Get -Uri "$base/releases?per_page=100" -Headers $headers
$staleReleases = @($releases | Where-Object { $_.id -ne $latest.id })
$removed = 0

foreach ($release in $staleReleases) {
    foreach ($asset in $release.assets) {
        if ($asset.name -notmatch '(?i)(setup\.exe|\.msi|\.exe\.sig$|^latest\.json$)') {
            continue
        }

        Invoke-RestMethod -Method Delete -Uri "$base/releases/assets/$($asset.id)" -Headers $headers | Out-Null
        Write-Host "Removed stale release asset $($release.tag_name)/$($asset.name)"
        $removed++
    }
}

Write-Host "Kept latest release $($latest.tag_name); removed $removed older installer/updater assets."
