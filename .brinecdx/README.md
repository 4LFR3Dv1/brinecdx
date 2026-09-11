# BrineCDX overlay

BrineCDX is a downstream distribution of OpenAI Codex that keeps the Codex product surface and agent environment while progressively delegating execution authority to Brine.

## Architectural rule

Codex owns interaction and agent ergonomics. Brine owns durable execution truth where integrated.

The integration must not create two competing authorities for the same physical effect. A Codex-facing action that crosses the Brine boundary must be represented by one Brine-owned lifecycle and projected back into Codex UI/protocol events.

## Current integration seam

BCDX-00 uses Codex's existing remote exec-server abstraction instead of replacing the Codex model loop. When the fork is built with `BRINE_EXEC_SERVER_URL` set, the Brine URL takes precedence over upstream `CODEX_EXEC_SERVER_URL` and is selected as the remote execution/filesystem environment.

Because a configured remote environment suppresses the local one, Brine mode is fail-closed with respect to physical execution: connection failure must surface instead of silently running the command through Codex's local backend.

The Brine side still needs to implement the Codex exec-server protocol before the first end-to-end witness is acquired. See `EXEC_SERVER_CONTRACT.md` and issue #2.

## Upstream discipline

- `codex-upstream` points at the exact OpenAI Codex commit recorded in `.brinecdx/UPSTREAM`.
- `main` is that upstream commit plus BrineCDX-specific commits.
- Keep the downstream delta small and prefer existing Codex seams over invasive rewrites.
- Apache-2.0 notices from upstream remain intact.
