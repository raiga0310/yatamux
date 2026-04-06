[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Pane,

    [switch]$Json,

    [switch]$Snapshot,

    [int]$Lines = 200
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($Snapshot) {
    $args = @(
        'capture-pane',
        '--target', $Pane,
        '--lines', $Lines.ToString()
    )

    if ($Json) {
        $args += '--json'
    } else {
        $args += '--plain-text'
    }

    & yatamux @args
    exit $LASTEXITCODE
}

$streamArgs = @(
    'subscribe-pane',
    '--pane', $Pane
)

if ($Json) {
    $streamArgs += '--json'
}

& yatamux @streamArgs
exit $LASTEXITCODE
