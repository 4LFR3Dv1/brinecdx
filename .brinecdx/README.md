# BrineCDX overlay

BrineCDX is a downstream distribution of OpenAI Codex that keeps the Codex product surface and agent environment while progressively delegating execution authority to Brine.

## Architectural rule

Codex owns interaction and agent ergonomics. Brine owns durable execution truth where integrated.

The integration must not create two competing authorities for the same physical effect. A Codex-facing action that crosses the Brine boundary must be represented by one Brine-owned lifecycle and projected back into Codex UI/protocol events.

## Current integration seam

BCDX-00 uses Codex's existing remote exec-server abstraction instead of replacing the Codex model loop. When the fork is built with `BRINE_EXEC_SERVER_URL` set, the Brine URL takes precedence over upstream `CODEX_EXEC_SERVER_URL` and is selected as the remote execution/filesystem environment.

Because a configured remote environment suppresses the local one, Brine mode is fail-closed with respect to physical execution: connection failure must surface instead of silently running the command through Codex's local backend.

The agent runtime for BrineCDX is the Codex CLI built from this fork and signed in with ChatGPT. DeepSeek is not part of the BrineCDX runtime path; provider and model selection must be fixed by the BrineCDX launcher so a machine-wide default cannot change it.

The Brine side still needs to implement the Codex exec-server protocol before the first end-to-end witness is acquired. See `EXEC_SERVER_CONTRACT.md` and issue #2.

## Launcher

`scripts/brinecdx.sh` (plus `scripts/brinecdx.ps1` and `scripts/brinecdx.cmd` on Windows) starts the Codex CLI from this checkout against Brine:

```sh
export BRINE_EXEC_SERVER_URL=wss://brine.example/exec
export BRINE_EXEC_SERVER_TOKEN=...   # optional; Codex sends it as Authorization: Bearer
scripts/brinecdx.sh
```

The launcher never edits `~/.codex/config.toml`. BrineCDX-owned settings travel as `-c` overrides, so a machine configured for another provider keeps working unchanged:

| Setting | Value | Why |
| --- | --- | --- |
| `model` | `gpt-5.6-sol` | ChatGPT-backed Codex model |
| `model_provider` | `openai` | ChatGPT auth instead of a local provider |
| `model_reasoning_effort` | `medium` | matches the ChatGPT-era default |
| `model_catalog_json` | `codex-rs/models-manager/models.json` | escapes machine-wide catalogs that do not list the Codex models |
| `forced_login_method` | `chatgpt` | escapes API-key-only login policies |

Arguments after the launcher are forwarded to Codex, and a later `-c key=value` wins over the launcher defaults. `BRINE_EXEC_SERVER_URL` is required and must not be `none`; BrineCDX fails closed instead of executing through the host-local backend. `BRINECDX_DRY_RUN=1` prints the resolved argv without running anything, and `BRINECDX_CODEX_BIN` runs an already-built Codex binary instead of `cargo run`.

Sign-in is a one-time prerequisite: run `codex login` and choose ChatGPT.

## Upstream discipline

- `codex-upstream` points at the exact OpenAI Codex commit recorded in `.brinecdx/UPSTREAM`.
- `main` is that upstream commit plus BrineCDX-specific commits.
- Keep the downstream delta small and prefer existing Codex seams over invasive rewrites.
- Apache-2.0 notices from upstream remain intact.
