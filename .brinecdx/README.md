# BrineCDX overlay

BrineCDX is a downstream distribution of OpenAI Codex that keeps the Codex product surface and agent environment while progressively delegating execution authority to Brine.

## Architectural rule

Codex owns interaction and agent ergonomics. Brine owns durable execution truth where integrated.

The integration must not create two competing authorities for the same physical effect. A Codex-facing action that crosses the Brine boundary must be represented by one Brine-owned lifecycle and projected back into Codex UI/protocol events.

## Upstream discipline

- `codex-upstream` tracks the exact OpenAI Codex commit recorded in `.brinecdx/UPSTREAM`.
- `main` is that upstream commit plus BrineCDX-specific commits.
- Keep the downstream delta small and prefer existing Codex seams over invasive rewrites.
- Apache-2.0 notices from upstream remain intact.
