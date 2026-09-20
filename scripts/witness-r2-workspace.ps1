param(
    [int]$RuntimePort = 4555,
    [int]$AppServerPort = 4233,
    [string]$CargoTargetDir
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$factoryRoot = Split-Path -Parent $repoRoot
if (-not $CargoTargetDir) {
    $CargoTargetDir = Join-Path $factoryRoot '.cargo-target'
}
$CargoTargetDir = [IO.Path]::GetFullPath($CargoTargetDir)
$debugDir = Join-Path $CargoTargetDir 'debug'
$runtimeExe = Join-Path $debugDir 'brine-runtime-server.exe'
$appServerExe = Join-Path $debugDir 'codex-app-server.exe'

foreach ($path in @($runtimeExe, $appServerExe)) {
    if (-not (Test-Path $path)) {
        throw "Required R2 binary not found: $path. Run scripts\validate-r2-workspace.ps1 -BuildBinaries first."
    }
}

foreach ($port in @($RuntimePort, $AppServerPort)) {
    $listener = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
    if ($listener) {
        throw "Witness port $port is already in use by PID $($listener[0].OwningProcess)."
    }
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$witnessRoot = Join-Path $HOME ".brinecdx\witness\r2-$stamp"
$workspaceRoot = Join-Path $witnessRoot 'repo'
$statePath = Join-Path $witnessRoot 'runtime.json'
$summaryPath = Join-Path $witnessRoot 'summary.json'
$logDir = Join-Path $witnessRoot 'logs'
New-Item -ItemType Directory -Force -Path (Join-Path $workspaceRoot 'src'), $logDir | Out-Null

function Invoke-Git {
    param([string[]]$Arguments)

    & git -C $workspaceRoot @Arguments | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Wait-Listener {
    param([int]$Port, [int]$TimeoutSeconds = 10)

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
            return
        }
        Start-Sleep -Milliseconds 150
    }
    throw "Timed out waiting for listener on port $Port."
}

function Wait-PortClosed {
    param([int]$Port, [int]$TimeoutSeconds = 10)

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)) {
            return
        }
        Start-Sleep -Milliseconds 150
    }
    throw "Timed out waiting for port $Port to close."
}

function Start-Runtime {
    $process = Start-Process -FilePath $runtimeExe -ArgumentList @(
        ('"' + $statePath + '"'),
        "127.0.0.1:$RuntimePort"
    ) -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logDir 'runtime.stdout.log') -RedirectStandardError (Join-Path $logDir 'runtime.stderr.log')
    Wait-Listener -Port $RuntimePort
    return $process
}

function Start-AppServer {
    $previous = $env:BRINECDX_RUNTIME_AUTHORITY_ADDR
    $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = "127.0.0.1:$RuntimePort"
    try {
        $process = Start-Process -FilePath $appServerExe -ArgumentList @(
            '--listen',
            "ws://127.0.0.1:$AppServerPort"
        ) -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logDir 'app-server.stdout.log') -RedirectStandardError (Join-Path $logDir 'app-server.stderr.log')
    }
    finally {
        if ($null -eq $previous) {
            Remove-Item Env:BRINECDX_RUNTIME_AUTHORITY_ADDR -ErrorAction SilentlyContinue
        }
        else {
            $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = $previous
        }
    }
    Wait-Listener -Port $AppServerPort
    return $process
}

function Connect-AppServer {
    $socket = [System.Net.WebSockets.ClientWebSocket]::new()
    [void]$socket.ConnectAsync(
        [Uri]"ws://127.0.0.1:$AppServerPort",
        [Threading.CancellationToken]::None
    ).GetAwaiter().GetResult()
    if ($socket.State -ne [System.Net.WebSockets.WebSocketState]::Open) {
        throw "WebSocket failed to open: $($socket.State)"
    }
    return $socket
}

function Send-WsJson {
    param($Socket, $Object)

    $json = $Object | ConvertTo-Json -Depth 20 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($json)
    $segment = [ArraySegment[byte]]::new($bytes)
    [void]$Socket.SendAsync(
        $segment,
        [System.Net.WebSockets.WebSocketMessageType]::Text,
        $true,
        [Threading.CancellationToken]::None
    ).GetAwaiter().GetResult()
}

function Receive-WsJson {
    param($Socket)

    if ($Socket.State -ne [System.Net.WebSockets.WebSocketState]::Open) {
        throw "WebSocket is not open: $($Socket.State)"
    }

    $memory = [IO.MemoryStream]::new()
    try {
        do {
            $buffer = New-Object byte[] 65536
            $segment = [ArraySegment[byte]]::new($buffer)
            $result = $Socket.ReceiveAsync(
                $segment,
                [Threading.CancellationToken]::None
            ).GetAwaiter().GetResult()
            if ($result.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) {
                throw 'WebSocket closed while waiting for JSON-RPC response.'
            }
            $memory.Write($buffer, 0, $result.Count)
        }
        until ($result.EndOfMessage)

        [Text.Encoding]::UTF8.GetString($memory.ToArray()) | ConvertFrom-Json
    }
    finally {
        $memory.Dispose()
    }
}

function Invoke-WsRpc {
    param(
        $Socket,
        [int]$Id,
        [string]$Method,
        $Params
    )

    Send-WsJson $Socket @{
        jsonrpc = '2.0'
        id = $Id
        method = $Method
        params = $Params
    }

    while ($true) {
        $message = Receive-WsJson $Socket
        if ($message.id -eq $Id) {
            if ($message.error) {
                throw "$Method failed: $($message.error | ConvertTo-Json -Compress)"
            }
            return $message
        }
    }
}

function Initialize-Client {
    param($Socket, [int]$Id)

    [void](Invoke-WsRpc $Socket $Id 'initialize' @{
        clientInfo = @{
            name = 'brine-r2-witness'
            title = 'Brine R2 Workspace Witness'
            version = '1'
        }
    })
    Send-WsJson $Socket @{
        jsonrpc = '2.0'
        method = 'initialized'
    }
}

function Start-WitnessThread {
    param($Socket, [int]$Id)

    $response = Invoke-WsRpc $Socket $Id 'thread/start' @{
        cwd = $workspaceRoot
        ephemeral = $true
    }
    return $response.result.thread.id
}

function Read-RuntimeState {
    Get-Content $statePath -Raw | ConvertFrom-Json
}

function First-Value {
    param($Map)
    return @($Map.PSObject.Properties.Value)[0]
}

function Workspace-Material {
    param($State, [string]$WorkspaceId)
    $property = $State.workspace_material.PSObject.Properties[$WorkspaceId]
    if (-not $property) {
        throw "workspace_material missing for $WorkspaceId"
    }
    return $property.Value
}

function Assert-Equal {
    param($Actual, $Expected, [string]$Message)
    if ($Actual -ne $Expected) {
        throw "$Message. Expected '$Expected', got '$Actual'."
    }
}

function Snapshot-Summary {
    param(
        [string]$Label,
        [string]$SessionId,
        $State,
        [string]$WorkspaceId,
        [string]$WorkId
    )

    $material = Workspace-Material $State $WorkspaceId
    [pscustomobject]@{
        label = $Label
        session_id = $SessionId
        runtime_revision = [uint64]$State.revision
        workspace_id = $WorkspaceId
        work_id = $WorkId
        material_revision = [uint64]$material.material_revision
        structural_revision = [uint64]$material.structural_revision
        material_digest = $material.observation.material_digest
        structure_digest = $material.observation.structure.digest
        changed_paths = @($material.observation.changed_paths)
        symbols = @($material.observation.structure.symbols | ForEach-Object { "$($_.kind):$($_.name)" })
    }
}

$runtimeProcess = $null
$appServerProcess = $null
$socket = $null
$observations = @()

try {
    Invoke-Git @('init')
    Invoke-Git @('config', 'user.email', 'brine-r2-witness@example.invalid')
    Invoke-Git @('config', 'user.name', 'Brine R2 Witness')
    Set-Content -Path (Join-Path $workspaceRoot 'src\lib.rs') -Value "pub fn run() {}" -Encoding UTF8
    Set-Content -Path (Join-Path $workspaceRoot 'README.md') -Value "one" -Encoding UTF8
    Invoke-Git @('add', '.')
    Invoke-Git @('commit', '-m', 'initial')

    $runtimeProcess = Start-Runtime
    $appServerProcess = Start-AppServer
    $socket = Connect-AppServer
    Initialize-Client $socket 1

    $s1 = Start-WitnessThread $socket 101
    $state1 = Read-RuntimeState
    $workspace1 = First-Value $state1.workspaces
    $work1 = @($state1.works.PSObject.Properties.Value | Where-Object { $_.workspace_id -eq $workspace1.id -and $_.key -eq 'root' })[0]
    if (-not $work1) { throw 'Root Work was not created for the witness workspace.' }
    $m1 = Workspace-Material $state1 $workspace1.id
    Assert-Equal $m1.material_revision 1 'Initial material revision'
    Assert-Equal $m1.structural_revision 1 'Initial structural revision'
    $observations += Snapshot-Summary 'initial' $s1 $state1 $workspace1.id $work1.id

    $s2 = Start-WitnessThread $socket 102
    $state2 = Read-RuntimeState
    $m2 = Workspace-Material $state2 $workspace1.id
    Assert-Equal $state2.revision $state1.revision 'Unchanged attach changed runtime revision'
    Assert-Equal $m2.material_revision $m1.material_revision 'Unchanged attach changed material revision'
    Assert-Equal $m2.structural_revision $m1.structural_revision 'Unchanged attach changed structural revision'
    $observations += Snapshot-Summary 'unchanged' $s2 $state2 $workspace1.id $work1.id

    Set-Content -Path (Join-Path $workspaceRoot 'README.md') -Value "two" -Encoding UTF8
    $s3 = Start-WitnessThread $socket 103
    $state3 = Read-RuntimeState
    $m3 = Workspace-Material $state3 $workspace1.id
    Assert-Equal $m3.material_revision ($m2.material_revision + 1) 'README edit did not advance material exactly once'
    Assert-Equal $m3.structural_revision $m2.structural_revision 'README edit incorrectly advanced structural revision'
    $observations += Snapshot-Summary 'external-doc-edit' $s3 $state3 $workspace1.id $work1.id

    Set-Content -Path (Join-Path $workspaceRoot 'src\lib.rs') -Value @(
        'pub fn run() {}',
        'pub struct State;'
    ) -Encoding UTF8
    $s4 = Start-WitnessThread $socket 104
    $state4 = Read-RuntimeState
    $m4 = Workspace-Material $state4 $workspace1.id
    Assert-Equal $m4.material_revision ($m3.material_revision + 1) 'Source edit did not advance material exactly once'
    Assert-Equal $m4.structural_revision ($m3.structural_revision + 1) 'Source edit did not advance structural revision'
    if (-not @($m4.observation.structure.symbols | Where-Object { $_.name -eq 'State' }).Count) {
        throw 'Structural index did not observe the State symbol.'
    }
    $observations += Snapshot-Summary 'external-source-edit' $s4 $state4 $workspace1.id $work1.id

    $socket.Dispose()
    $socket = $null
    Stop-Process -Id $appServerProcess.Id -Force -ErrorAction SilentlyContinue
    Stop-Process -Id $runtimeProcess.Id -Force -ErrorAction SilentlyContinue
    Wait-PortClosed -Port $AppServerPort
    Wait-PortClosed -Port $RuntimePort

    $runtimeProcess = Start-Runtime
    $appServerProcess = Start-AppServer
    $socket = Connect-AppServer
    Initialize-Client $socket 2

    $s5 = Start-WitnessThread $socket 105
    $state5 = Read-RuntimeState
    $m5 = Workspace-Material $state5 $workspace1.id
    Assert-Equal (First-Value $state5.workspaces).id $workspace1.id 'Workspace identity changed across process restart'
    $work5 = @($state5.works.PSObject.Properties.Value | Where-Object { $_.workspace_id -eq $workspace1.id -and $_.key -eq 'root' })[0]
    Assert-Equal $work5.id $work1.id 'Root Work identity changed across process restart'
    Assert-Equal $state5.revision $state4.revision 'Restart with unchanged material changed runtime revision'
    Assert-Equal $m5.material_revision $m4.material_revision 'Restart changed material revision'
    Assert-Equal $m5.structural_revision $m4.structural_revision 'Restart changed structural revision'
    $observations += Snapshot-Summary 'runtime-restart' $s5 $state5 $workspace1.id $work1.id

    Invoke-Git @('reset', '--hard', 'HEAD')
    $s6 = Start-WitnessThread $socket 106
    $state6 = Read-RuntimeState
    $m6 = Workspace-Material $state6 $workspace1.id
    Assert-Equal $m6.material_revision ($m5.material_revision + 1) 'git reset did not advance material exactly once'
    Assert-Equal $m6.structural_revision ($m5.structural_revision + 1) 'git reset did not restore the prior structural state as a new transition'
    if (@($m6.observation.structure.symbols | Where-Object { $_.name -eq 'State' }).Count) {
        throw 'Structural index still reports State after git reset removed it.'
    }
    $observations += Snapshot-Summary 'git-reset' $s6 $state6 $workspace1.id $work1.id

    $summary = [pscustomobject]@{
        result = 'PASS'
        created_at = (Get-Date).ToString('o')
        runtime_protocol = 2
        workspace_id = $workspace1.id
        work_id = $work1.id
        artifact_root = $witnessRoot
        observations = $observations
    }
    $summary | ConvertTo-Json -Depth 20 | Set-Content -Path $summaryPath -Encoding UTF8

    Write-Host ""
    Write-Host 'R2 PRODUCT WITNESS: PASS'
    Write-Host "Workspace: $($workspace1.id)"
    Write-Host "Work:      $($work1.id)"
    Write-Host "Evidence:  $summaryPath"
    $observations | Format-Table label, runtime_revision, material_revision, structural_revision -AutoSize
}
finally {
    if ($socket) {
        try { $socket.Dispose() } catch {}
    }
    if ($appServerProcess -and -not $appServerProcess.HasExited) {
        Stop-Process -Id $appServerProcess.Id -Force -ErrorAction SilentlyContinue
    }
    if ($runtimeProcess -and -not $runtimeProcess.HasExited) {
        Stop-Process -Id $runtimeProcess.Id -Force -ErrorAction SilentlyContinue
    }
}
