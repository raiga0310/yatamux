[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Alias,

    [string]$Role,

    [string]$Dir,

    [ValidateSet('vertical', 'horizontal')]
    [string]$Direction = 'vertical',

    [string]$Target = '0',

    [string]$BootstrapCommand
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$splitArgs = @(
    'split-pane',
    '--target', $Target,
    '--direction', $Direction
)

if ($Dir) {
    $splitArgs += @('--dir', $Dir)
}

$splitOutput = & yatamux @splitArgs
$splitText = ($splitOutput | Out-String).Trim()

if ($splitText -notmatch 'Created pane (?<pane>\d+)') {
    throw "Could not parse pane ID from split-pane output: $splitText"
}

$pane = $Matches['pane']

$metaArgs = @(
    'set-pane-meta',
    '--pane', $pane,
    '--alias', $Alias
)

if ($Role) {
    $metaArgs += @('--role', $Role)
}

& yatamux @metaArgs | Out-Null

if ($BootstrapCommand) {
    & yatamux send-keys --pane $Alias --enter --raw $BootstrapCommand | Out-Null
}

Write-Output $pane
