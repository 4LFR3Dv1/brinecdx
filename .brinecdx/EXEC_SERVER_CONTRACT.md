# Brine execution boundary

BCDX-00 uses Codex's existing remote exec-server seam rather than replacing the Codex agent loop.

When `BRINE_EXEC_SERVER_URL` is set, BrineCDX selects that WebSocket environment before the upstream `CODEX_EXEC_SERVER_URL`. A configured remote environment suppresses the local environment, so transport failure does not silently fall back to local physical execution.

## Ownership

Codex owns:

- TUI and session ergonomics;
- skills, plugins and repository instructions;
- model/provider loop and context management;
- human-readable projection of tool activity.

Brine owns, for capabilities crossing this boundary:

- authorization to perform the physical effect;
- durable dispatch/start/settlement lifecycle;
- stdout/stderr/process evidence;
- cancellation/reconciliation;
- the decision whether a retry is safe after ambiguous state.

Codex process IDs, thread IDs and tool-call IDs are correlation metadata. They are not Brine authority or proof that an effect happened.

## Minimum Codex exec-server surface

The first Brine adapter should implement the existing `codex-exec-server-protocol` methods, beginning with:

- `initialize`
- `initialized`
- `environment/info`
- `environment/status`
- `process/start`
- `process/read`
- `process/write`
- `process/signal`
- `process/terminate`
- `process/output`
- `process/exited`
- `process/closed`

Normal Codex repository work also needs the filesystem surface used by the selected remote environment:

- `fs/readFile`
- `fs/open`
- `fs/readBlock`
- `fs/close`
- `fs/writeFile`
- `fs/createDirectory`
- `fs/getMetadata`
- `fs/canonicalize`
- `fs/readDirectory`
- `fs/walk`
- `fs/remove`
- `fs/copy`

Capability discovery and executor-owned HTTP can be added after the first vertical slice rather than faked.

## Process mapping

For a Codex `process/start` request:

1. Validate and canonicalize the requested workspace/cwd inside the Brine workspace boundary.
2. Persist a Brine intent/authorization record before physical execution.
3. Materialize one Brine ToolEffect/process lifecycle.
4. Acknowledge the Codex logical `processId` only after Brine has a durable process/effect identity capable of being reconciled.
5. Stream observed stdout/stderr as Codex `process/output` notifications while retaining Brine's durable evidence separately.
6. Publish `process/exited`/`process/closed` from durable Brine settlement, not from transport disappearance.

A lost WebSocket is not proof that a process failed or did not run. Reconnect/recovery must query Brine durable state before any redispatch.

## Mutation mapping

Filesystem mutation must eventually cross the same authority boundary. The Codex UI may continue to show normal diffs and edited-file cells, but `fs/writeFile`, remove/copy and patch materialization must not execute independently of Brine settlement when Brine mode is active.

## Fail-closed rule

`BRINE_EXEC_SERVER_URL` is an explicit request for Brine-owned execution. If that environment cannot initialize, BrineCDX must surface the failure. It must not execute the command through the host-local Codex backend as an implicit fallback.

## First witness

The first accepted end-to-end witness is intentionally read-only:

```text
DeepSeek Flash / Codex loop
        -> exec_command
        -> remote Codex exec-server protocol
        -> Brine ToolEffect
        -> one physical command
        -> durable settlement
        -> streamed output
        -> normal Codex TUI cell
```

Only after this is acquired should `apply_patch` and other mutation paths be admitted.
