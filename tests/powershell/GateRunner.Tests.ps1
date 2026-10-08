$modulePath = Join-Path $PSScriptRoot '..\..\scripts\gates\GateRunner.psm1'
if (Test-Path -LiteralPath $modulePath) {
    Import-Module $modulePath -Force
}

Describe 'Invoke-CheckedCommand' {
    It 'accepts a native command with exit code zero' {
        $result = Invoke-CheckedCommand -FilePath 'cmd.exe' -ArgumentList @('/D', '/C', 'exit 0') -GateName 'success gate'
        $result.ExitCode | Should Be 0
        $result.Status | Should Be 'Passed'
    }

    It 'reports the gate name and nonzero exit code' {
        $message = $null
        try {
            Invoke-CheckedCommand -FilePath 'cmd.exe' -ArgumentList @('/D', '/C', 'exit 7') -GateName 'red gate'
        }
        catch {
            $message = $_.Exception.Message
        }
        $message | Should Match 'red gate'
        $message | Should Match '7'
    }

    It 'fails if a required executable is missing' {
        $message = $null
        try {
            Invoke-CheckedCommand -FilePath 'flux-code-m0-missing-command.exe' -ArgumentList @() -GateName 'required gate'
        }
        catch {
            $message = $_.Exception.Message
        }
        $message | Should Match 'required gate'
        $message | Should Match 'not found'
    }

    It 'reports a missing optional executable as skipped' {
        $result = Invoke-CheckedCommand -FilePath 'flux-code-m0-optional-command.exe' -ArgumentList @() -GateName 'optional gate' -Optional
        $result.Status | Should Be 'Skipped'
    }

    It 'fails when an installed optional executable returns a nonzero exit code' {
        $message = $null
        try {
            Invoke-CheckedCommand -FilePath 'cmd.exe' -ArgumentList @('/D', '/C', 'exit 9') -GateName 'installed optional gate' -Optional
        }
        catch {
            $message = $_.Exception.Message
        }
        $message | Should Match 'installed optional gate'
        $message | Should Match '9'
    }
}

Describe 'Invoke-GateSequence' {
    It 'does not execute gates after the first failure' {
        $markerPath = Join-Path $TestDrive 'second-gate-ran.txt'
        $checks = @(
            [PSCustomObject]@{ Name = 'first gate'; FilePath = 'cmd.exe'; ArgumentList = @('/D', '/C', 'exit 4'); Optional = $false },
            [PSCustomObject]@{ Name = 'second gate'; FilePath = 'cmd.exe'; ArgumentList = @('/D', '/C', "echo reached > `"$markerPath`""); Optional = $false }
        )
        $message = $null
        try {
            Invoke-GateSequence -Checks $checks
        }
        catch {
            $message = $_.Exception.Message
        }
        $message | Should Match 'first gate'
        Test-Path -LiteralPath $markerPath | Should Be $false
    }
}
