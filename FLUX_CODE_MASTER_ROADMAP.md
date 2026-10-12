# Flux Code / Flux Hands — master roadmap

Flux Code and Flux Hands are names for the same product. Flux Hands is the current application/release name.

## Product goal

Build a dependable desktop development workspace that combines project editing, local and remote models, and an OpenHands agent. The user should be able to inspect and control the agent's work, understand what data leaves the machine, and verify results. Android is a remote companion: model inference and project tools run on a configured server.

The product should grow from the existing Tauri, Rust, TypeScript, Monaco, llama.cpp and OpenHands REST integration. Do not rewrite or split components without evidence that it improves reliability or testability.

## Development rules

1. Audit existing behavior before implementing a replacement.
2. Distinguish implemented code, automated verification, real runtime verification, and release verification.
3. A task is complete only when its stated acceptance checks pass; report limits and known failures.
4. Prioritize reproducible P0 defects over new breadth.
5. Preserve workspace data, secrets, and user control when changing storage, permissions, or updates.
6. Keep release artifacts traceable to a source commit and version; do not call an unsigned local installer an official updater release.

## Stages

| Stage | Scope | Exit evidence |
| --- | --- | --- |
| 0. Audit | Architecture, feature status, build and scenario evidence, risk register | `PROJECT_AUDIT.md` reflects source and actual checks |
| 1. Core reliability | Install/launch, project and file flows, editor, persistence, error recovery | Critical scenarios pass on a clean supported Windows environment |
| 2. Safety and clarity | Permissions, action confirmation, diff/recovery, privacy and cost notices | Agent access and changes are inspectable and enforceable |
| 3. Models | Local/remote provider paths, model switching, credentials, timeouts and cancellation | Provider paths tested end to end and switching preserves the task |
| 4. Agent task workflow | Task lifecycle, tools, verification, correction and reports | Result claims are backed by actual command/test evidence |
| 5. Public release | Signed installers, updater, performance, docs and release checks | Reproducible release assets and documented smoke test |
| 6–8. Adoption and expansion | User research, integrations, extensions and sustainable business options | Pursue only after the core product is stable and validated |

## Priority order

- **P0:** stable install and startup; project/file editing; model/agent correctness; saved user state; comprehensible failures; bounded agent access; change review and risky-action approval; tested critical flows; user documentation.
- **P1:** onboarding, unified provider behavior, cancellation/recovery, verification, task tracking, privacy diagnostics.
- **P2:** advanced routing, extensions, team workflows, automation and commercial features.

The current factual status, risks, and next implementation order live in `PROJECT_AUDIT.md` and `IMPLEMENTATION_STATUS.md`. Each release must update `CHANGELOG.md` and record what was actually built and tested.
