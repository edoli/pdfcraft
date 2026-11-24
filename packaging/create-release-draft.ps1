#Requires -Version 7.0
<#
.SYNOPSIS
  Create a GitHub release draft from a non-scheduled PdfCraft Release run.
.DESCRIPTION
  Requires an authenticated GitHub CLI. By default, selects the newest manual Release
  run or push to the release branch. -RunId selects a specific Release run.
  Downloads release artifacts uploaded by successful jobs.
  Use -WhatIf to list uploaded release artifacts and the run start time.
  Job success is checked when creating a draft.
  Leaves the extracted files available for inspection or manual upload.
.EXAMPLE
  ./packaging/create-release-draft.ps1 -WhatIf
.EXAMPLE
  ./packaging/create-release-draft.ps1 -Repository edoli/pdfcraft
#>
[CmdletBinding(SupportsShouldProcess)]
param(
  [ValidatePattern('^[a-zA-Z0-9-]+/[a-zA-Z0-9_.-]+$')]
  [string] $Repository = 'edoli/pdfcraft',
  [ValidateRange('Positive')]
  [long] $RunId,
  [ValidateRange('Positive')]
  [int] $RunAttempt,
  [switch] $SkipExisting
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Get-Command gh -ErrorAction Stop | Out-Null

function Invoke-Gh([string[]] $Arguments) {
  & gh @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "gh $($Arguments[0]) $($Arguments[1]) failed (exit code $LASTEXITCODE)."
  }
}

if ($RunAttempt -and -not $RunId) { throw '-RunAttempt requires -RunId.' }

if ($RunId) {
  $runFields = '{attempt:.run_attempt,createdAt:.created_at,databaseId:.id,headSha:.head_sha,startedAt:.run_started_at,status,url:.html_url,path,event,headBranch:.head_branch}'
  $run = Invoke-Gh @('api', "repos/$Repository/actions/runs/$RunId", '--jq', $runFields) | ConvertFrom-Json
  if ($run.path -ne '.github/workflows/release.yml' -or
      ($run.event -ne 'workflow_dispatch' -and -not ($run.event -eq 'push' -and $run.headBranch -eq 'release'))) {
    throw "Run $RunId is not a manual Release run or a Release run triggered by a push to the release branch."
  }
  if ($RunAttempt -and $run.attempt -ne $RunAttempt) {
    Write-Host "Release run $RunId attempt $RunAttempt was superseded by attempt $($run.attempt)."
    return
  }
} else {
  $runArguments = @('run', 'list', '-R', $Repository, '--workflow', 'release.yml', '--limit', '1',
    '--json', 'attempt,createdAt,databaseId,headSha,startedAt,status,url')
  $manualRuns = Invoke-Gh ($runArguments + @('--event', 'workflow_dispatch')) | ConvertFrom-Json
  $pushRuns = Invoke-Gh ($runArguments + @('--event', 'push', '--branch', 'release')) | ConvertFrom-Json
  $run = $manualRuns, $pushRuns | Where-Object { $_ } |
    Sort-Object -Property createdAt, databaseId -Descending | Select-Object -First 1
}

if (-not $run) { throw "No Release run found in $Repository." }

$createdAt = ([DateTimeOffset]$run.createdAt).ToLocalTime().ToString('yyyy-MM-dd HH:mm:ss zzz')
Write-Host "Release run $($run.databaseId) at $($run.headSha) ($($run.status))"
Write-Host "  Run created      $createdAt"
if ($run.startedAt) {
  $startedAt = ([DateTimeOffset]$run.startedAt).ToLocalTime().ToString('yyyy-MM-dd HH:mm:ss zzz')
  Write-Host "  Attempt $($run.attempt) started  $startedAt"
}
Write-Host "  $($run.url)"
if (-not $WhatIfPreference -and $run.status -ne 'completed') {
  throw "The latest Release run is $($run.status)."
}

$artifactEndpoint = "repos/$Repository/actions/runs/$($run.databaseId)/artifacts?per_page=100"
$releaseArtifacts = @(Invoke-Gh @('api', $artifactEndpoint, '--paginate', '--jq', '.artifacts[] | {id,name,expired}') |
  ForEach-Object { $_ | ConvertFrom-Json } |
  Where-Object { $_.name -like 'release-*' } | Sort-Object Name)

if ($WhatIfPreference) {
  Write-Host "Uploaded release artifacts ($($releaseArtifacts.Count))"
  foreach ($artifact in $releaseArtifacts) {
    $status = if ($artifact.expired) { ' (expired)' } else { '' }
    Write-Host "  $($artifact.name)$status"
  }
  return
}

if ($releaseArtifacts.Count -eq 0) { throw 'No release artifacts were uploaded.' }

# The artifact API gives the run ID but not the job ID. Match the upload's artifact ID
# against logs from successful jobs, including jobs carried forward from earlier attempts.
$jobsEndpoint = "repos/$Repository/actions/runs/$($run.databaseId)/jobs?filter=latest&per_page=100"
$successfulJobs = @(Invoke-Gh @('api', $jobsEndpoint, '--paginate', '--jq', '.jobs[] | {id,name,conclusion}') |
  ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.conclusion -eq 'success' })
$artifactJobs = @{}
foreach ($job in $successfulJobs) {
  try {
    $uploads = Invoke-Gh @('api', "repos/$Repository/actions/jobs/$($job.id)/logs") |
      Select-String -Pattern '\bArtifact release-\S+ successfully finalized\. Artifact ID (?<id>\d+)\s*$'
    foreach ($upload in $uploads) {
      $artifactJobs[[long]$upload.Matches[0].Groups['id'].Value] = $job.name
    }
  } catch {
    Write-Warning "Could not verify artifact uploads for $($job.name)."
  }
}

Write-Host 'Release artifacts'
foreach ($artifact in $releaseArtifacts) {
  $jobName = $artifactJobs[[long]$artifact.id]
  if ($artifact.expired) {
    Write-Host "  Skip     $($artifact.name) (expired)"
  } elseif ($jobName) {
    Write-Host "  Include  $($artifact.name) ($jobName)"
  } else {
    Write-Host "  Skip     $($artifact.name) (no verified upload from a successful job)"
  }
}
$artifacts = @($releaseArtifacts | Where-Object { -not $_.expired -and $artifactJobs.ContainsKey([long]$_.id) })
if ($artifacts.Count -eq 0) { throw 'No downloadable release artifacts from successful jobs were verified.' }
if (-not $PSCmdlet.ShouldProcess($run.url, 'Download artifacts and create a GitHub release draft')) { return }

$directory = Join-Path ([System.IO.Path]::GetTempPath()) "pdfcraft-release-$($run.databaseId)-$([guid]::NewGuid())"
Write-Host "Extracting release assets to $directory"
$downloadArguments = @('run', 'download', [string]$run.databaseId, '-R', $Repository, '--dir', $directory)
foreach ($artifact in $artifacts) { $downloadArguments += @('--name', $artifact.name) }
Invoke-Gh $downloadArguments

$assets = @(Get-ChildItem -LiteralPath $directory -Recurse -File)
if ($assets.Count -eq 0) { throw "No files were extracted to $directory." }
$duplicates = @($assets | Group-Object Name | Where-Object Count -gt 1)
if ($duplicates.Count -gt 0) { throw "Duplicate release asset names: $($duplicates.Name -join ', ')." }

# The package names reflect any version override supplied when the workflow was dispatched.
$versionPattern = '(?<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?)'
$versions = @($assets | ForEach-Object {
  if ($_.Name -match "^pdfcraft-web-$versionPattern\.zip$" -or
      $_.Name -match "^pdfcraft(?:-cli|-server)?-$versionPattern-(?:windows-\w+\.msi|macos-\w+\.(?:dmg|zip)|(?:freebsd|linux)-\w+\.(?:tar\.gz|AppImage|flatpak|pkg\.tar\.zst))$") {
    $Matches.version
  }
} | Sort-Object -Unique)
if ($versions.Count -ne 1) { throw "Expected one build version in the extracted packages. Found: $($versions -join ', ')." }
$version = $versions[0]
$tag = "v$version"

$existingReleases = @(Invoke-Gh @('api', "repos/$Repository/releases?per_page=100", '--paginate', '--jq', '.[] | {tag_name,html_url}') |
  ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.tag_name -ceq $tag })
if ($existingReleases.Count -gt 0) {
  $message = "$tag already has a release or draft at $($existingReleases[0].html_url)."
  if ($SkipExisting) {
    Write-Host $message
    return
  }
  throw "$message Extracted files are available at $directory."
}

# GitHub ignores --target when the tag already exists.
$tagRef = "refs/tags/$tag"
$escapedTag = [uri]::EscapeDataString($tag)
$tagRefs = @(Invoke-Gh @('api', "repos/$Repository/git/matching-refs/tags/$escapedTag", '--jq', '.[].ref'))
if ($tagRefs -ccontains $tagRef) {
  $tagCommit = Invoke-Gh @('api', "repos/$Repository/commits/$escapedTag", '--jq', '.sha')
  if ($tagCommit -ne $run.headSha) { throw "$tag points to $tagCommit. This Release run built $($run.headSha)." }
}

$releaseArguments = @('release', 'create', $tag, '-R', $Repository, '--draft', '--generate-notes',
  '--title', "PdfCraft $tag", '--target', $run.headSha)
if (($version -split '\+', 2)[0].Contains('-')) { $releaseArguments += '--prerelease' }

$currentRun = Invoke-Gh @('run', 'view', [string]$run.databaseId, '-R', $Repository, '--json', 'attempt,status') | ConvertFrom-Json
if ($currentRun.attempt -ne $run.attempt -or $currentRun.status -ne 'completed') {
  throw 'The Release run was restarted while preparing the draft. Check the latest attempt before running this script again.'
}
Write-Host "Creating $tag as a draft with $($assets.Count) files."
Invoke-Gh ($releaseArguments + @('--') + $assets.FullName)
