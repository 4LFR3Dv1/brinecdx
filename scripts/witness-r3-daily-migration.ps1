param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceRoot,
    [string]$StatePath,
    [string]$R2BackupPath,
    [string]$CargoTargetDir,
    [int]$RuntimePort = 4545,
    [int]$AppServerPort = 4222
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$factoryRoot = Split-Path -Parent $repoRoot

if (-not $StatePath) { $StatePath = Join-Path $HOME '.brinecdx\runtime.json' }
if (-not $R2BackupPath) { $R2BackupPath = Join-Path $HOME '.brinecdx\runtime.r2.backup.json' }
if (-not $CargoTargetDir) { $CargoTargetDir = Join-Path $factoryRoot '.cargo-target' }

$WorkspaceRoot = [IO.Path]::GetFullPath($WorkspaceRoot)
$StatePath = [IO.Path]::GetFullPath($StatePath)
$R2BackupPath = [IO.Path]::GetFullPath($R2BackupPath)
$CargoTargetDir = [IO.Path]::GetFullPath($CargoTargetDir)

$runtimeExe = Join-Path $CargoTargetDir 'debug\brine-runtime-server.exe'
$appServerExe = Join-Path $CargoTargetDir 'debug\codex-app-server.exe'

foreach ($path in @($WorkspaceRoot, $StatePath, $R2BackupPath, $runtimeExe, $appServerExe)) {
    if (-not (Test-Path $path)) { throw "Required path does not exist: $path" }
}

foreach ($port in @($RuntimePort, $AppServerPort)) {
    $listener = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
    if ($listener) {
        throw "Port $port is already in use by PID $($listener[0].OwningProcess). Stop daily BrineCDX before migration witness."
    }
}

$currentHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $StatePath).Hash
$backupHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $R2BackupPath).Hash
if ($currentHash -ne $backupHash) {
    throw "Daily runtime state no longer matches the R2 backup. Refusing automatic migration. Current=$currentHash Backup=$backupHash"
}

$before = Get-Content -LiteralPath $R2BackupPath -Raw | ConvertFrom-Json
$beforeRevision = [uint64]$before.revision

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$witnessRoot = Join-Path $HOME ".brinecdx\witness\r3-daily-$stamp"
$logDir = Join-Path $witnessRoot 'logs'
$summaryPath = Join-Path $witnessRoot 'summary.json'
$preMigrationCopy = Join-Path $witnessRoot 'runtime.pre-r3.json'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
Copy-Item -LiteralPath $StatePath -Destination $preMigrationCopy

function Wait-Listener {
    param([int]$Port, [int]$TimeoutSeconds = 10)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) { return }
        Start-Sleep -Milliseconds 150
    }
    throw "Timed out waiting for listener on port $Port."
}

function Wait-PortClosed {
    param([int]$Port, [int]$TimeoutSeconds = 10)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)) { return }
        Start-Sleep -Milliseconds 150
    }
    throw "Timed out waiting for port $Port to close."
}

function Start-Runtime {
    $args = @{
        FilePath = $runtimeExe
        ArgumentList = @(('"' + $StatePath + '"'), "127.0.0.1:$RuntimePort")
        WorkingDirectory = $repoRoot
        WindowStyle = 'Hidden'
        PassThru = $true
        RedirectStandardOutput = (Join-Path $logDir 'runtime.stdout.log')
        RedirectStandardError = (Join-Path $logDir 'runtime.stderr.log')
    }
    $process = Start-Process @args
    Wait-Listener -Port $RuntimePort
    return $process
}

function Start-AppServer {
    $previous = $env:BRINECDX_RUNTIME_AUTHORITY_ADDR
    $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = "127.0.0.1:$RuntimePort"
    try {
        $args = @{
            FilePath = $appServerExe
            ArgumentList = @('--listen', "ws://127.0.0.1:$AppServerPort")
            WorkingDirectory = $repoRoot
            WindowStyle = 'Hidden'
            PassThru = $true
            RedirectStandardOutput = (Join-Path $logDir 'app-server.stdout.log')
            RedirectStandardError = (Join-Path $logDir 'app-server.stderr.log')
        }
        $process = Start-Process @args
    }
    finally {
        if ($null -eq $previous) {
            Remove-Item Env:BRINECDX_RUNTIME_AUTHORITY_ADDR -ErrorAction SilentlyContinue
        } else {
            $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = $previous
        }
    }
    Wait-Listener -Port $AppServerPort
    return $process
}

function Send-WsJson {
    param($Socket, $Object)
    $json = $Object | ConvertTo-Json -Depth 30 -Compress
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
        } until ($result.EndOfMessage)
        return ([Text.Encoding]::UTF8.GetString($memory.ToArray()) | ConvertFrom-Json)
    }
    finally {
        $memory.Dispose()
    }
}

function Invoke-WsRpc {
    param($Socket, [int]$Id, [string]$Method, $Params)
    Send-WsJson $Socket @{
        jsonrpc = '2.0'
        id = $Id
        method = $Method
        params = $Params
    }
    while ($true) {
        $message = Receive-WsJson $Socket
        if ($message.id -eq $Id) {
            if ($message.error) { throw "$Method failed: $($message.error | ConvertTo-Json -Compress)" }
            return $message
        }
    }
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

function Invoke-BrineRuntime {
    param([string]$Operation, $Payload)

    $request = @{ operation = $Operation; payload = $Payload } | ConvertTo-Json -Depth 30 -Compress
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $client.Connect('127.0.0.1', $RuntimePort)
        $stream = $client.GetStream()
        $utf8NoBom = [Text.UTF8Encoding]::new($false)
        $writer = [IO.StreamWriter]::new($stream, $utf8NoBom, 65536, $true)
        $writer.NewLine = [char]10
        $writer.WriteLine($request)
        $writer.Flush()
        $client.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)

        $reader = [IO.StreamReader]::new($stream, $utf8NoBom)
        $responseText = $reader.ReadToEnd()
        if ([string]::IsNullOrWhiteSpace($responseText)) { throw "$Operation failed: runtime returned an empty response" }
        $response = $responseText | ConvertFrom-Json
        if ($response.error) { throw "$Operation failed: $($response.error)" }
        return $response.value
    }
    finally {
        $client.Dispose()
    }
}

function Map-Value {
    param($Map, [string]$Key)
    $property = $Map.PSObject.Properties[$Key]
    if (-not $property) { return $null }
    return $property.Value
}

$runtimeProcess = $null
$appServerProcess = $null
$socket = $null

try {
    $runtimeProcess = Start-Runtime
    $appServerProcess = Start-AppServer
    $socket = Connect-AppServer

    [void](Invoke-WsRpc $socket 1 'initialize' @{
        clientInfo = @{
            name = 'brine-r3-daily-migration-witness'
            title = 'Brine R3 Daily Migration Witness'
            version = '1'
        }
    })
    Send-WsJson $socket @{ jsonrpc = '2.0'; method = 'initialized' }

    $thread = Invoke-WsRpc $socket 2 'thread/start' @{
        cwd = $WorkspaceRoot
        ephemeral = $true
    }
    $threadId = [string]$thread.result.thread.id

    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    $after = $null
    $workId = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $after = Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json
        $workId = Map-Value $after.thread_bindings $threadId
        if ($workId) { break }
        Start-Sleep -Milliseconds 150
    }
    if (-not $workId) { throw "R3 thread $threadId did not materialize a durable Work binding." }

    $workId = [string]$workId
    $afterWork = Map-Value $after.works $workId
    if (-not $afterWork) { throw "R3 Work binding points to missing Work $workId." }

    $workspaceId = [string]$afterWork.workspace_id
    $afterWorkspace = Map-Value $after.workspaces $workspaceId
    if (-not $afterWorkspace) { throw "R3 Work points to missing Workspace $workspaceId." }

    $beforeWork = Map-Value $before.works $workId
    $beforeWorkspace = Map-Value $before.workspaces $workspaceId

    if (-not $beforeWork) { throw "Work continuity failed: R3 Work $workId did not exist in the R2 backup." }
    if (-not $beforeWorkspace) { throw "Workspace continuity failed: R3 Workspace $workspaceId did not exist in the R2 backup." }
    if ([string]$beforeWork.key -ne 'root' -or [string]$afterWork.key -ne 'root') {
        throw 'Expected daily migration to continue the root Work.'
    }
    if ($afterWork.parent_work) { throw "Root Work unexpectedly acquired parent_work=$($afterWork.parent_work)." }
    if ([string]$afterWork.assigned_thread -ne $threadId) {
        throw 'Root Work was not assigned to the migration witness thread.'
    }

    $stateProjection = Invoke-BrineRuntime 'State' @{
        workspace_id = $workspaceId
        work_id = $workId
        since_revision = $beforeRevision
    }
    if (-not $stateProjection.work_graph) { throw 'R3 RuntimeState did not expose WorkGraph.' }
    if ([string]$stateProjection.work_graph.active_work -ne $workId) {
        throw 'R3 WorkGraph active_work does not match the durable root Work.'
    }
    $graphNode = @($stateProjection.work_graph.nodes | Where-Object { $_.id -eq $workId })[0]
    if (-not $graphNode) { throw 'R3 WorkGraph does not contain the durable root Work.' }

    $beforeMaterial = Map-Value $before.workspace_material $workspaceId
    $afterMaterial = Map-Value $after.workspace_material $workspaceId

    $summary = [pscustomobject]@{
        result = 'PASS'
        created_at = (Get-Date).ToString('o')
        workspace_root = $WorkspaceRoot
        state_path = $StatePath
        r2_backup_path = $R2BackupPath
        pre_migration_copy = $preMigrationCopy
        state_hash_before = $currentHash
        backup_hash = $backupHash
        thread_id = $threadId
        workspace_id = $workspaceId
        workspace_same = ([string]$beforeWorkspace.id -eq [string]$afterWorkspace.id)
        workspace_key_before = [string]$beforeWorkspace.key
        workspace_key_after = [string]$afterWorkspace.key
        work_id = $workId
        work_same = ([string]$beforeWork.id -eq [string]$afterWork.id)
        work_key = [string]$afterWork.key
        work_status = [string]$afterWork.status
        assigned_thread = [string]$afterWork.assigned_thread
        runtime_revision_before = $beforeRevision
        runtime_revision_after = [uint64]$after.revision
        material_revision_before = if ($beforeMaterial) { [uint64]$beforeMaterial.material_revision } else { $null }
        material_revision_after = if ($afterMaterial) { [uint64]$afterMaterial.material_revision } else { $null }
        structural_revision_before = if ($beforeMaterial) { [uint64]$beforeMaterial.structural_revision } else { $null }
        structural_revision_after = if ($afterMaterial) { [uint64]$afterMaterial.structural_revision } else { $null }
        workgraph_nodes = @($stateProjection.work_graph.nodes).Count
        workgraph_active_work = [string]$stateProjection.work_graph.active_work
    }

    $summary | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    Write-Host ''
    Write-Host 'R3 DAILY MIGRATION WITNESS: PASS'
    Write-Host "WorkspaceSame      : $($summary.workspace_same)"
    Write-Host "WorkSame           : $($summary.work_same)"
    Write-Host "WorkspaceId        : $workspaceId"
    Write-Host "RootWorkId         : $workId"
    Write-Host "WorkStatus         : $($summary.work_status)"
    Write-Host "AssignedThread     : $($summary.assigned_thread)"
    Write-Host "RuntimeRevision    : $($summary.runtime_revision_before) -> $($summary.runtime_revision_after)"
    Write-Host "MaterialRevision   : $($summary.material_revision_before) -> $($summary.material_revision_after)"
    Write-Host "StructuralRevision : $($summary.structural_revision_before) -> $($summary.structural_revision_after)"
    Write-Host "WorkGraphNodes     : $($summary.workgraph_nodes)"
    Write-Host "Evidence           : $summaryPath"
}
finally {
    if ($socket) {
        try {
            if ($socket.State -eq [System.Net.WebSockets.WebSocketState]::Open) {
                [void]$socket.CloseAsync(
                    [System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure,
                    'witness complete',
                    [Threading.CancellationToken]::None
                ).GetAwaiter().GetResult()
            }
        } catch {}
        $socket.Dispose()
    }
    if ($appServerProcess -and -not $appServerProcess.HasExited) {
        Stop-Process -Id $appServerProcess.Id -Force -ErrorAction SilentlyContinue
        Wait-PortClosed -Port $AppServerPort
    }
    if ($runtimeProcess -and -not $runtimeProcess.HasExited) {
        Stop-Process -Id $runtimeProcess.Id -Force -ErrorAction SilentlyContinue
        Wait-PortClosed -Port $RuntimePort
    }
}
