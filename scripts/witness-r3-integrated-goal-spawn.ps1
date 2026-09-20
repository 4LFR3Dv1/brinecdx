param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceRoot,
    [string]$CargoTargetDir,
    [int]$RuntimePort = 4567,
    [int]$AppServerPort = 4234,
    [int]$TimeoutSeconds = 180
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$factoryRoot = Split-Path -Parent $repoRoot
if (-not $CargoTargetDir) { $CargoTargetDir = Join-Path $factoryRoot '.cargo-target' }

$WorkspaceRoot = [IO.Path]::GetFullPath($WorkspaceRoot)
$CargoTargetDir = [IO.Path]::GetFullPath($CargoTargetDir)
$runtimeExe = Join-Path $CargoTargetDir 'debug\brine-runtime-server.exe'
$appServerExe = Join-Path $CargoTargetDir 'debug\codex-app-server.exe'

foreach ($path in @($WorkspaceRoot, $runtimeExe, $appServerExe)) {
    if (-not (Test-Path $path)) { throw "Required path does not exist: $path" }
}
foreach ($port in @($RuntimePort, $AppServerPort)) {
    $listener = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
    if ($listener) { throw "Witness port $port is already in use by PID $($listener[0].OwningProcess)." }
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$witnessRoot = Join-Path $HOME ".brinecdx\witness\r3-integrated-$stamp"
$statePath = Join-Path $witnessRoot 'runtime.json'
$summaryPath = Join-Path $witnessRoot 'summary.json'
$logDir = Join-Path $witnessRoot 'logs'
$sqliteHome = Join-Path $witnessRoot 'sqlite'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
New-Item -ItemType Directory -Force -Path $sqliteHome | Out-Null

function Wait-Listener {
    param(
        [int]$Port,
        [int]$Timeout = 10,
        $Process = $null,
        [string]$ErrorLog = $null
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) { return }

        if ($Process) {
            $Process.Refresh()
            if ($Process.HasExited) {
                $details = ''
                if ($ErrorLog -and (Test-Path $ErrorLog)) {
                    $rawDetails = Get-Content -LiteralPath $ErrorLog -Raw -ErrorAction SilentlyContinue
                    if ($null -ne $rawDetails) {
                        $details = ([string]$rawDetails).Trim()
                    }
                }
                if ($details) {
                    throw "Process exited before listener on port $Port (exit=$($Process.ExitCode)). stderr: $details"
                }
                throw "Process exited before listener on port $Port (exit=$($Process.ExitCode))."
            }
        }

        Start-Sleep -Milliseconds 150
    }

    $details = ''
    if ($ErrorLog -and (Test-Path $ErrorLog)) {
        $rawDetails = Get-Content -LiteralPath $ErrorLog -Raw -ErrorAction SilentlyContinue
        if ($null -ne $rawDetails) {
            $details = ([string]$rawDetails).Trim()
        }
    }
    if ($details) {
        throw "Timed out waiting for listener on port $Port after $($Timeout)s. stderr: $details"
    }
    throw "Timed out waiting for listener on port $Port after $($Timeout)s."
}

function Wait-PortClosed {
    param([int]$Port, [int]$Timeout = 10)
    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)) { return }
        Start-Sleep -Milliseconds 150
    }
    throw "Timed out waiting for port $Port to close."
}

function Start-Runtime {
    $args = @{
        FilePath = $runtimeExe
        ArgumentList = @(('"' + $statePath + '"'), "127.0.0.1:$RuntimePort")
        WorkingDirectory = $repoRoot
        WindowStyle = 'Hidden'
        PassThru = $true
        RedirectStandardOutput = (Join-Path $logDir 'runtime.stdout.log')
        RedirectStandardError = (Join-Path $logDir 'runtime.stderr.log')
    }
    $process = Start-Process @args
    try {
        Wait-Listener -Port $RuntimePort -Timeout 10 -Process $process -ErrorLog (Join-Path $logDir 'runtime.stderr.log')
        return $process
    }
    catch {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        throw
    }
}

function Start-AppServer {
    $previousRuntime = $env:BRINECDX_RUNTIME_AUTHORITY_ADDR
    $previousSqliteHome = $env:CODEX_SQLITE_HOME
    $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = "127.0.0.1:$RuntimePort"
    $env:CODEX_SQLITE_HOME = $sqliteHome
    $process = $null
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
        Wait-Listener -Port $AppServerPort -Timeout 40 -Process $process -ErrorLog (Join-Path $logDir 'app-server.stderr.log')
        return $process
    }
    catch {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        throw
    }
    finally {
        if ($null -eq $previousRuntime) {
            Remove-Item Env:BRINECDX_RUNTIME_AUTHORITY_ADDR -ErrorAction SilentlyContinue
        } else {
            $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = $previousRuntime
        }
        if ($null -eq $previousSqliteHome) {
            Remove-Item Env:CODEX_SQLITE_HOME -ErrorAction SilentlyContinue
        } else {
            $env:CODEX_SQLITE_HOME = $previousSqliteHome
        }
    }
}

function Send-WsJson {
    param($Socket, $Object)
    $json = $Object | ConvertTo-Json -Depth 40 -Compress
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
                throw 'WebSocket closed while waiting for JSON-RPC data.'
            }
            $memory.Write($buffer, 0, $result.Count)
        } until ($result.EndOfMessage)
        return ([Text.Encoding]::UTF8.GetString($memory.ToArray()) | ConvertFrom-Json)
    }
    finally {
        $memory.Dispose()
    }
}

$script:PendingMessages = @()

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
            if ($message.error) { throw "$Method failed: $($message.error | ConvertTo-Json -Depth 20 -Compress)" }
            return $message
        }
        $script:PendingMessages += $message
    }
}

function Wait-Notification {
    param($Socket, [string]$Method, [string]$ThreadId, [int]$Timeout)

    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    while ([DateTime]::UtcNow -lt $deadline) {
        for ($i = 0; $i -lt $script:PendingMessages.Count; $i++) {
            $message = $script:PendingMessages[$i]
            if ($message.method -eq $Method) {
                $messageThread = [string]$message.params.threadId
                if (-not $ThreadId -or -not $messageThread -or $messageThread -eq $ThreadId) {
                    if ($i -eq 0) {
                        $script:PendingMessages = @($script:PendingMessages | Select-Object -Skip 1)
                    } else {
                        $script:PendingMessages = @(
                            $script:PendingMessages[0..($i - 1)]
                            $script:PendingMessages[($i + 1)..($script:PendingMessages.Count - 1)]
                        )
                    }
                    return $message
                }
            }
        }

        $message = Receive-WsJson $Socket
        if ($message.method -eq $Method) {
            $messageThread = [string]$message.params.threadId
            if (-not $ThreadId -or -not $messageThread -or $messageThread -eq $ThreadId) { return $message }
        }
        $script:PendingMessages += $message
    }
    throw "Timed out waiting for notification $Method."
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

function Read-State {
    if (-not (Test-Path $statePath)) { return $null }
    return (Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json)
}

function Map-Value {
    param($Map, [string]$Key)
    if (-not $Map) { return $null }
    $property = $Map.PSObject.Properties[$Key]
    if (-not $property) { return $null }
    return $property.Value
}

function Wait-ThreadBinding {
    param([string]$ThreadId, [int]$Timeout)
    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    while ([DateTime]::UtcNow -lt $deadline) {
        $state = Read-State
        if ($state) {
            $workId = Map-Value $state.thread_bindings $ThreadId
            if ($workId) {
                return [pscustomobject]@{ state = $state; work_id = [string]$workId }
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Thread $ThreadId did not acquire a durable Work binding."
}

function Wait-ChildWork {
    param([string]$WorkspaceId, [string]$RootWorkId, [string]$RootThreadId, [int]$Timeout)
    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    while ([DateTime]::UtcNow -lt $deadline) {
        $state = Read-State
        if ($state -and $state.works) {
            foreach ($property in $state.works.PSObject.Properties) {
                $work = $property.Value
                if (
                    [string]$work.workspace_id -eq $WorkspaceId -and
                    [string]$work.parent_work -eq $RootWorkId -and
                    [string]$work.assigned_thread -ne $RootThreadId
                ) {
                    return [pscustomobject]@{
                        state = $state
                        work = $work
                    }
                }
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw 'No child Work materialized from the real spawn_agent turn.'
}

function Wait-RootComplete {
    param([string]$RootWorkId, [int]$Timeout)
    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    while ([DateTime]::UtcNow -lt $deadline) {
        $state = Read-State
        if ($state) {
            $work = Map-Value $state.works $RootWorkId
            if ($work -and [string]$work.status -eq 'complete') {
                return [pscustomobject]@{ state = $state; work = $work }
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw 'Root Work did not project the completed app-server goal after parent turn idle.'
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
            name = 'brine-r3-integrated-witness'
            title = 'Brine R3 Integrated Goal Spawn Witness'
            version = '1'
        }
    })
    Send-WsJson $socket @{ jsonrpc = '2.0'; method = 'initialized' }

    $threadResponse = Invoke-WsRpc $socket 2 'thread/start' @{
        cwd = $WorkspaceRoot
        ephemeral = $false
    }
    $rootThreadId = [string]$threadResponse.result.thread.id
    $rootBinding = Wait-ThreadBinding -ThreadId $rootThreadId -Timeout 15
    $rootWorkId = [string]$rootBinding.work_id
    $rootWork = Map-Value $rootBinding.state.works $rootWorkId
    if (-not $rootWork) { throw "Root Work $rootWorkId missing from runtime state." }
    $workspaceId = [string]$rootWork.workspace_id

    $objective = 'R3 integrated witness: goal projection plus one real spawned child Work'
    $goalActive = Invoke-WsRpc $socket 3 'thread/goal/set' @{
        threadId = $rootThreadId
        objective = $objective
        status = 'active'
    }
    if ([string]$goalActive.result.goal.status -ne 'active') {
        throw "Goal did not enter active state: $($goalActive.result | ConvertTo-Json -Compress)"
    }

    $prompt = @'
This is a bounded runtime witness. You MUST call spawn_agent exactly once before replying.
Give the child this exact task: Reply with exactly READY. Do not call tools. Do not modify files.
Do not use shell, git, file editing, network, or any other tools yourself.
After spawn_agent returns, wait for the child if a wait tool is available, then reply with exactly DONE.
Do not spawn more than one child.
'@

    $turnResponse = Invoke-WsRpc $socket 4 'turn/start' @{
        threadId = $rootThreadId
        input = @(@{
            type = 'text'
            text = $prompt
            textElements = @()
        })
    }
    $turnId = [string]$turnResponse.result.turn.id

    $child = Wait-ChildWork -WorkspaceId $workspaceId -RootWorkId $rootWorkId -RootThreadId $rootThreadId -Timeout $TimeoutSeconds
    $childWork = $child.work
    $childWorkId = [string]$childWork.id
    $childThreadId = [string]$childWork.assigned_thread

    $goalComplete = Invoke-WsRpc $socket 5 'thread/goal/set' @{
        threadId = $rootThreadId
        status = 'complete'
    }
    if ([string]$goalComplete.result.goal.status -ne 'complete') {
        throw "Goal did not enter complete state: $($goalComplete.result | ConvertTo-Json -Compress)"
    }

    $turnCompleted = Wait-Notification -Socket $socket -Method 'turn/completed' -ThreadId $rootThreadId -Timeout $TimeoutSeconds
    $rootComplete = Wait-RootComplete -RootWorkId $rootWorkId -Timeout 15

    $goalGet = Invoke-WsRpc $socket 6 'thread/goal/get' @{ threadId = $rootThreadId }
    if ([string]$goalGet.result.goal.status -ne 'complete') {
        throw 'App-server goal no longer reports complete after the parent turn finished.'
    }

    $finalState = $rootComplete.state
    $finalRoot = $rootComplete.work
    $finalChild = Map-Value $finalState.works $childWorkId
    if (-not $finalChild) { throw 'Child Work disappeared from durable runtime state.' }
    if ([string]$finalChild.parent_work -ne $rootWorkId) { throw 'Child Work parent does not point to root Work.' }
    if ($childWorkId -eq $rootWorkId) { throw 'Child Work reused root Work identity.' }

    $summary = [pscustomobject]@{
        result = 'PASS'
        created_at = (Get-Date).ToString('o')
        workspace_root = $WorkspaceRoot
        runtime_state = $statePath
        codex_sqlite_home = $sqliteHome
        root_thread_id = $rootThreadId
        root_work_id = $rootWorkId
        workspace_id = $workspaceId
        goal_objective = $objective
        goal_status = [string]$goalGet.result.goal.status
        root_work_status = [string]$finalRoot.status
        root_work_objective = [string]$finalRoot.objective
        parent_turn_id = $turnId
        turn_completion_status = [string]$turnCompleted.params.turn.status
        child_thread_id = $childThreadId
        child_work_id = $childWorkId
        child_parent_work = [string]$finalChild.parent_work
        child_status = [string]$finalChild.status
        runtime_revision = [uint64]$finalState.revision
        workgraph_nodes = @($finalState.works.PSObject.Properties.Value).Count
    }
    $summary | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    [void](Invoke-WsRpc $socket 7 'thread/goal/clear' @{ threadId = $rootThreadId })

    Write-Host ''
    Write-Host 'R3 INTEGRATED GOAL/SPAWN WITNESS: PASS'
    Write-Host "Workspace:       $workspaceId"
    Write-Host "Root Thread:     $rootThreadId"
    Write-Host "Root Work:       $rootWorkId"
    Write-Host "Goal Status:     $($summary.goal_status)"
    Write-Host "Root Work Status:$($summary.root_work_status)"
    Write-Host "Child Thread:    $childThreadId"
    Write-Host "Child Work:      $childWorkId"
    Write-Host "Child Parent:    $($summary.child_parent_work)"
    Write-Host "Child Status:    $($summary.child_status)"
    Write-Host "Runtime Revision:$($summary.runtime_revision)"
    Write-Host "SQLite Evidence: $sqliteHome"
    Write-Host "Evidence:        $summaryPath"
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
