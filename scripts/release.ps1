#Requires -Version 7.0
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

function Invoke-Native {
    param(
        [Parameter(Mandatory)] [string] $Program,
        [Parameter(Mandatory)] [string[]] $Arguments
    )
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    Invoke-Native jj @('git', 'fetch')

    $empty = (Invoke-Native jj @('log', '-r', '@', '--no-graph', '-T', 'empty')).Trim()
    if ($empty -ne 'true') {
        throw 'the working copy is not clean; commit or discard your changes first'
    }

    Invoke-Native gh @('auth', 'status')

    $runCommunique = (Read-Host 'Run Communiqué? [y/N]') -match '^(?i)y'
    $Version = (Read-Host 'Tag version (e.g. 0.2.0)').Trim().TrimStart('v')
    if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw "version must be x.y.z (got '$Version')"
    }
    $tag = "v$Version"

    $tags = Invoke-Native jj @('tag', 'list')
    if ($tags -match "(?m)^$([regex]::Escape($tag))(?:\s|$)") {
        throw "tag $tag already exists"
    }

    # The manifest only moves forward; refuse a version below the newest tag.
    $latest = $tags |
        ForEach-Object { ($_ -split ':')[0].Trim() } |
        Where-Object { $_ -match '^v\d+\.\d+\.\d+$' } |
        ForEach-Object { [version]($_.Substring(1)) } |
        Sort-Object |
        Select-Object -Last 1
    if ($latest -and ([version]$Version -lt $latest)) {
        throw "version $Version is older than the latest tag v$latest"
    }

    gh release view $tag --json isDraft 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) {
        throw "release $tag already exists"
    }

    if ($runCommunique) {
        if (-not $env:OPENAI_API_KEY) {
            $env:OPENAI_API_KEY = (infisical secrets get OPENCODE_GO_API_KEY --plain --silent).Trim()
            if ($LASTEXITCODE -ne 0 -or -not $env:OPENAI_API_KEY) {
                throw 'OPENAI_API_KEY is not set and OPENCODE_GO_API_KEY could not be read from Infisical'
            }
        }
        if (-not $env:GITHUB_TOKEN) {
            $env:GITHUB_TOKEN = (gh auth token 2>$null)
        }
        if (-not $env:OPENAI_CUSTOM_HEADERS) {
            $env:OPENAI_CUSTOM_HEADERS = (@{
                'x-opencode-session' = [guid]::NewGuid().ToString()
            } | ConvertTo-Json -Compress)
        }
        Invoke-Native mise @('exec', '--', 'communique', 'generate', $tag, '--concise', '--changelog')
    }

    # Validate the section and take the notes before changing Cargo.toml or
    # pushing a tag: a failure here must not leave a dirty checkout or a tag
    # without a draft release.
    Invoke-Native mise @('exec', '--', 'node', 'packaging/changelog.ts', $Version, '--check')
    $notes = (Invoke-Native mise @('exec', '--', 'node', 'packaging/changelog.ts', $Version)) -join "`n"

    $cargoPath = Join-Path $root 'Cargo.toml'
    $cargo = Get-Content $cargoPath -Raw
    $updated = $cargo -replace '(?m)^version = "\d+\.\d+\.\d+"', "version = `"$Version`""
    if ($updated -ne $cargo) {
        Set-Content $cargoPath $updated -NoNewline
    }
    Invoke-Native cargo @('metadata', '--offline', '--format-version', '1') | Out-Null

    Write-Host ''
    Invoke-Native jj @('diff', '--stat')

    $empty = (Invoke-Native jj @('log', '-r', '@', '--no-graph', '-T', 'empty')).Trim()
    if ($empty -ne 'true') {
        Invoke-Native jj @('commit', '-m', "release $tag")
    }

    Invoke-Native jj @('bookmark', 'set', 'main', '-r', '@-')
    Invoke-Native jj @('tag', 'set', $tag, '-r', '@-')
    Invoke-Native jj @('git', 'push', '-b', 'main')
    Invoke-Native jj @('git', 'push', '-t', $tag)

    $url = (Invoke-Native gh @('release', 'create', $tag, '--draft', '--title', "Kura $Version", '--notes', $notes)).Trim()

    Write-Host ''
    Write-Host "Draft: $url"
    Write-Host 'Review the draft on the web, then publish it. The workflow builds and attaches the installer.'
} finally {
    Pop-Location
}
