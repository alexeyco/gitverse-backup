# AGENTS.md

Guidelines for coding assistants working in this repository.

## What this repository is

See [README.md](README.md) for the repository purpose and usage.

## OpenSpec

When implementing a planned change, follow its artifacts under
`openspec/changes/`. Keep implementation aligned with its requirements,
design, and tasks.

## Architecture

Use Ports and Adapters (hexagonal) architecture. Keep application workflows
under `src/features/`, define small dependency interfaces in `src/ports.rs`,
and keep shared workflow data in `src/models.rs`. Implement external systems
under `src/adapters/`, and compose concrete adapters in `src/main.rs`. Keep
workflow code independent of concrete adapters and test it with Mockall mocks
of its ports. Keep scheduler time, waiting, and shutdown behavior injectable
through its runtime interface.

Keep unit tests in `#[cfg(test)]` modules beside the code they exercise. Reserve
`tests/` for end-to-end tests that exercise complete user-visible flows.

Add concise comments where they clarify a non-obvious invariant, ownership rule,
or external-system constraint. Explain why code does something; do not narrate
straightforward statements or comment every line.

## Configuration and credentials

Use environment variables for runtime configuration. Use `.env` only for
local development, keep it out of version control, and never write access
tokens or credentials to logs.

## Git rules

Use the branch naming convention in [CONTRIBUTING.md](CONTRIBUTING.md).
Do not stage, commit, push, or amend without an explicit user request.

## Documentation language

Write user-facing documentation, including `README.md`, `CONTRIBUTING.md`,
and files under `docs/`, in English. Keep this file in English as well.
