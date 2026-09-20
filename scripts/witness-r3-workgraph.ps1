param(
    [int]$RuntimePort = 4566,
    [string]$CargoTargetDir
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$factoryRoot = Split-Path -Parent $repoRoot
if (-not $CargoTargetDir) {
    $CargoTargetDir = Join-Path $factoryRoot '.cargo-target'
}
$CargoTargetDir = [IO.Path]::GetFullPath($CargoTargetDir)
$runtimeExe = Join-Path $CargoTargetDir 'debug\brine-runtime-server.exe'

if (-not (Test-Path $runtimeExe)) {
    throw "R3 runtime binary not found: $runtimeExe. Run scripts\validate-r3-workgraph.ps1 -BuildBinaries first."
}

$listener = Get-NetTCPConnection -LocalPort $RuntimePort -State Listen -ErrorAction SilentlyContinue
if ($listener) {
    throw "Witness port $RuntimePort is already in use by PID $($listener[0].OwningProcess)."
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$witnessRoot = Join-Path $HOME ".brinecdx\witness\r3-$stamp"
$statePath = Join-Path $witnessRoot 'runtime.json'
$summaryPath = Join-Path $witnessRoot 'summary.json'
$logDir = Join-Path $witnessRoot 'logs'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

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
    $startArgs = @{
        FilePath = $runtimeExe
        ArgumentList = @(('"' + $statePath + '"'), "127.0.0.1:$RuntimePort")
        WorkingDirectory = $repoRoot
        WindowStyle = 'Hidden'
        PassThru = $true
        RedirectStandardOutput = (Join-Path $logDir 'runtime.stdout.log')
        RedirectStandardError = (Join-Path $logDir 'runtime.stderr.log')
    }
    $process = Start-Process @startArgs
    Wait-Listener -Port $RuntimePort
    return $process
}

function Invoke-BrineRuntime {
    param(
        [string]$Operation,
        $Payload
    )

    $request = @{
        operation = $Operation
        payload = $Payload
    } | ConvertTo-Json -Depth 30 -Compress

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
        if ([string]::IsNullOrWhiteSpace($responseText)) {
            throw "$Operation failed: runtime returned an empty response"
        }
        $response = $responseText | ConvertFrom-Json
        if ($response.error) {
            throw "$Operation failed: $($response.error)"
        }
        return $response.value
    }
    finally {
        $client.Dispose()
    }
}

function Assert-Equal {
    param($Actual, $Expected, [string]$Message)
    if ($Actual -ne $Expected) {
        throw "$Message. Expected '$Expected', got '$Actual'."
    }
}

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw $Message
    }
}

function Attach-Work {
    param(
        [string]$SessionId,
        [string]$Objective,
        [string]$ParentSessionId = $null,
        $SinceRevision = $null
    )

    Invoke-BrineRuntime 'Attach' @{
        session_id = $SessionId
        workspace_key = 'witness:r3-workgraph'
        workspace_aliases = @()
        repository_identity = 'witness:r3-workgraph'
        work_key = ''
        objective = $Objective
        parent_session_id = $ParentSessionId
        since_revision = $SinceRevision
        material = $null
    }
}

function Update-Work {
    param(
        [string]$SessionId,
        $Objective,
        $Status,
        $SinceRevision
    )

    Invoke-BrineRuntime 'UpdateWork' @{
        session_id = $SessionId
        objective = $Objective
        status = $Status
        since_revision = $SinceRevision
    }
}

$runtimeProcess = $null
$observations = @()

try {
    $runtimeProcess = Start-Runtime

    $rootSession = 'r3-root-session'
    $childSession = 'r3-child-session'

    $root = Attach-Work -SessionId $rootSession -Objective 'root objective'
    Assert-Equal $root.protocol_version 3 'Runtime protocol version'
    Assert-Equal $root.state.work.key 'root' 'Root Work key'
    Assert-True ($null -eq $root.state.work.parent_work) 'Root Work unexpectedly has a parent'
    $rootWorkId = $root.attachment.work_id
    $workspaceId = $root.attachment.workspace_id
    $observations += [pscustomobject]@{
        label = 'root-attached'
        runtime_revision = $root.state.revision
        work_id = $rootWorkId
        parent_work = $null
        status = $root.state.work.status
    }

    $child = Attach-Work -SessionId $childSession -Objective 'child objective' -ParentSessionId $rootSession -SinceRevision $root.state.revision
    $childWorkId = $child.attachment.work_id
    Assert-True ($childWorkId -ne $rootWorkId) 'Child reused root Work identity'
    Assert-Equal $child.state.work.parent_work $rootWorkId 'Child parent Work'
    Assert-Equal $child.state.work.status 'active' 'Child initial status'
    Assert-Equal @($child.state.work_graph.nodes).Count 2 'WorkGraph node count after child attach'
    $observations += [pscustomobject]@{
        label = 'child-attached'
        runtime_revision = $child.state.revision
        work_id = $childWorkId
        parent_work = $child.state.work.parent_work
        status = $child.state.work.status
    }

    $completed = Update-Work -SessionId $childSession -Objective $null -Status 'complete' -SinceRevision $child.state.revision
    Assert-Equal $completed.attachment.work_id $childWorkId 'Child Work identity after completion'
    Assert-Equal $completed.state.work.status 'complete' 'Child completion status'
    $observations += [pscustomobject]@{
        label = 'child-complete'
        runtime_revision = $completed.state.revision
        work_id = $childWorkId
        parent_work = $completed.state.work.parent_work
        status = $completed.state.work.status
    }

    [void](Invoke-BrineRuntime 'Detach' $childSession)

    Stop-Process -Id $runtimeProcess.Id -Force -ErrorAction SilentlyContinue
    Wait-PortClosed -Port $RuntimePort
    $runtimeProcess = Start-Runtime

    $rebound = Attach-Work -SessionId $childSession -Objective 'child objective' -SinceRevision $completed.state.revision
    Assert-Equal $rebound.attachment.workspace_id $workspaceId 'Workspace identity after runtime restart'
    Assert-Equal $rebound.attachment.work_id $childWorkId 'Child Work identity after runtime restart'
    Assert-Equal $rebound.state.work.parent_work $rootWorkId 'Child parent after runtime restart'
    Assert-Equal $rebound.state.work.status 'complete' 'Attach incorrectly reactivated completed Work'
    $observations += [pscustomobject]@{
        label = 'child-rebound'
        runtime_revision = $rebound.state.revision
        work_id = $childWorkId
        parent_work = $rebound.state.work.parent_work
        status = $rebound.state.work.status
    }

    $reactivated = Update-Work -SessionId $childSession -Objective $null -Status 'active' -SinceRevision $rebound.state.revision
    Assert-Equal $reactivated.attachment.work_id $childWorkId 'Child Work identity after reactivation'
    Assert-Equal $reactivated.state.work.status 'active' 'Child reactivation status'
    $observations += [pscustomobject]@{
        label = 'child-reactivated'
        runtime_revision = $reactivated.state.revision
        work_id = $childWorkId
        parent_work = $reactivated.state.work.parent_work
        status = $reactivated.state.work.status
    }

    $newRootSession = 'r3-root-session-2'
    $rootRebound = Attach-Work -SessionId $newRootSession -Objective 'root objective' -SinceRevision $reactivated.state.revision
    Assert-Equal $rootRebound.attachment.work_id $rootWorkId 'New root thread did not continue root Work'
    Assert-Equal $rootRebound.attachment.workspace_id $workspaceId 'New root thread changed Workspace'
    $observations += [pscustomobject]@{
        label = 'root-new-thread'
        runtime_revision = $rootRebound.state.revision
        work_id = $rootWorkId
        parent_work = $rootRebound.state.work.parent_work
        status = $rootRebound.state.work.status
    }

    $rootComplete = Update-Work -SessionId $newRootSession -Objective 'goal-backed root objective' -Status 'complete' -SinceRevision $rootRebound.state.revision
    Assert-Equal $rootComplete.attachment.work_id $rootWorkId 'Root Work identity after goal projection'
    Assert-Equal $rootComplete.state.work.objective 'goal-backed root objective' 'Root goal objective projection'
    Assert-Equal $rootComplete.state.work.status 'complete' 'Root goal completion projection'
    Assert-Equal @($rootComplete.state.work_graph.nodes).Count 2 'Final WorkGraph node count'
    $observations += [pscustomobject]@{
        label = 'root-goal-complete'
        runtime_revision = $rootComplete.state.revision
        work_id = $rootWorkId
        parent_work = $rootComplete.state.work.parent_work
        status = $rootComplete.state.work.status
    }

    $summary = [pscustomobject]@{
        result = 'PASS'
        created_at = (Get-Date).ToString('o')
        runtime_protocol = 3
        workspace_id = $workspaceId
        root_work_id = $rootWorkId
        child_work_id = $childWorkId
        artifact_root = $witnessRoot
        observations = $observations
    }
    $summary | ConvertTo-Json -Depth 30 | Set-Content -Path $summaryPath -Encoding UTF8

    Write-Host ""
    Write-Host 'R3 WORKGRAPH WITNESS: PASS'
    Write-Host "Workspace:  $workspaceId"
    Write-Host "Root Work:  $rootWorkId"
    Write-Host "Child Work: $childWorkId"
    Write-Host "Evidence:   $summaryPath"
    $observations | Format-Table label, runtime_revision, work_id, parent_work, status -AutoSize
}
finally {
    if ($runtimeProcess -and -not $runtimeProcess.HasExited) {
        Stop-Process -Id $runtimeProcess.Id -Force -ErrorAction SilentlyContinue
    }
}
