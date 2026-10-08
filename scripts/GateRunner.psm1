function Invoke-CheckedCommand {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [string[]]$ArgumentList = @(),

        [Parameter(Mandatory = $true)]
        [string]$GateName,

        [switch]$Optional
    )

    $command = Get-Command -Name $FilePath -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1

    if ($null -eq $command) {
        if ($Optional) {
            Write-Host "SKIP optional gate '$GateName': executable '$FilePath' is not installed."
            return [PSCustomObject]@{
                Name = $GateName
                Status = 'Skipped'
                ExitCode = $null
            }
        }

        throw "Required gate '$GateName' cannot run: executable '$FilePath' was not found on PATH. Install it and retry."
    }

    Write-Host "RUN gate '$GateName': $($command.Source) $($ArgumentList -join ' ')"
    & $command.Source @ArgumentList
    $exitCode = $LASTEXITCODE

    if ($exitCode -ne 0) {
        throw "Gate '$GateName' failed with exit code $exitCode. Review the command output above and fix the reported error."
    }

    return [PSCustomObject]@{
        Name = $GateName
        Status = 'Passed'
        ExitCode = $exitCode
    }
}

function Invoke-GateSequence {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [object[]]$Checks
    )

    $results = @()
    foreach ($check in $Checks) {
        $arguments = @($check.ArgumentList)
        $result = Invoke-CheckedCommand `
            -FilePath ([string]$check.FilePath) `
            -ArgumentList $arguments `
            -GateName ([string]$check.Name) `
            -Optional:([bool]$check.Optional)
        $results += $result
    }

    return $results
}

Export-ModuleMember -Function Invoke-CheckedCommand, Invoke-GateSequence
