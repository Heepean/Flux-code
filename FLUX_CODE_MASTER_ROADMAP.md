# Flux Code / Flux Hands — master roadmap

Flux Code and Flux Hands are names for the same product. This roadmap describes the direction and release gates; the [project audit](PROJECT_AUDIT.md) and [implementation status](IMPLEMENTATION_STATUS.md) record what is actually present and verified. Code presence alone never counts as runtime proof.

## Product goal

Build a dependable Windows desktop workspace for projects, local GGUF models, remote model providers, and OpenHands-assisted task execution. Android is a remote-only companion: projects and agent tools run on an OpenHands server. The product should make the chosen model, data destination, permissions, and verification results clear.

## Product principles

- **Simple:** a first-time user can open a project and select a model without developer setup instructions.
- **Flexible:** local models and compatible remote providers share a coherent configuration flow. Free catalogs are refreshed from their provider and described with provider limits.
- **Useful:** agents use real project tools and report changed files and actual checks.
- **Trustworthy:** the user can see workspace scope, external data destinations, risky actions, and test results. Never claim an action succeeded without evidence.
- **Incremental:** stabilize the desktop product and mobile remote flow before building commercial features or a general extension platform.

## Current verified baseline (2026-10-12)

- Desktop development app builds; Rust test suite passes 9 tests.
- A temporary GGUF model ran through the native llama.cpp process manager, answered a completion, and shut down. Larger user models still depend on available RAM/VRAM.
- The app started OpenHands Agent Server 1.26.0 in a disposable workspace and completed a Kilo Free text task, including streamed UI events and clean server shutdown.
- That OpenHands startup logged a non-fatal browser-tool preload error (`Server.list_tools` missing); text tasks still worked. Tool actions and approval behavior are not yet verified end to end.
- Windows installers and Android APK can be built locally, but a clean Windows install and a physical Android 15+ launch have not been verified. The last public beta had Windows assets; its Android workflow failed during SDK setup.
- GitHub publication of the current 1.0.14 candidate is pending restored repository credentials.

See [PROJECT_AUDIT.md](PROJECT_AUDIT.md) for evidence and limitations, and [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for release gates.

## Staged plan

### Stage 0 — verified audit

Keep an evidence-backed feature matrix, architecture notes, reproducible build instructions, defect register, and environment limitations. Re-audit any claim when the runtime or release workflow changes.

**Gate:** major features have a status, reproducible scenario, severity, and next verification step.

### Stage 1 — dependable base product (P0)

- Verify first launch, folder selection, file listing/open/edit/save, errors, and restart persistence on Windows.
- Fix critical local model and OpenHands integration defects; test startup, model switch, request, failure, cancellation, and shutdown.
- Verify Android remote connection and a task on Android 15+ hardware or emulator.
- Improve process cleanup, dependency/runtime setup feedback, and diagnostic messages.
- Validate installers and updater without losing workspace or user settings.

**Gate:** clean install starts; a user can open a project, select a model, complete a task, inspect the result, restart, and retain expected settings; critical paths have regression coverage.

### Stage 2 — clarity and safety (P0/P1)

- Establish explicit workspace boundaries for both native commands and OpenHands runtime tools.
- Prove confirmation behavior: risky action is shown before execution, rejection prevents it, approval runs only the displayed action, and stop is responsive.
- Add a real diff and undo mechanism backed by captured file state, not a model request to reverse itself.
- Make privacy, provider destination, cost/rate limits, key storage, and action log visible at the point of use.

**Gate:** action permissions and rollback are demonstrated against the pinned runtime; supported data flows and limitations are documented.

### Stage 3 — predictable models (P0/P1)

- Keep one provider configuration shared across chat and model settings.
- Cover local model validation, memory/GPU limits, switching, stop, timeout, and cancellation.
- Cover Kilo/compatible provider catalog refresh, authentication, tool calling, rate limits, timeout, and unavailable models.
- Clearly distinguish local inference from data sent to a remote provider.

**Gate:** each supported provider has a repeatable real request and known failure test; selection and launch state agree.

### Stage 4 — task execution and verification (P0/P1)

- Keep the initial tool set small: read/search/edit files, inspect diff, run approved commands/tests, and revert captured changes.
- Stream task state, command output, approvals, changed files, and failures.
- Add bounded retry and cancellation behavior.
- Report exact checks run and separate passed, failed, skipped, and unavailable checks.

**Gate:** representative coding and file tasks finish with inspectable changes and evidence-based reports.

### Stage 5 — release readiness (P0)

- Build and sign Windows installers and updater metadata in CI; build the Android artifact for supported devices.
- Test clean install, upgrade, interrupted setup, recovery, process cleanup, and preservation of user data.
- Publish install guide, first-run guide, model/provider support, troubleshooting, privacy and license notices, changelog, and known issues.
- Record commit, tag, hashes, test results, install evidence, and public links for each candidate.

**Gate:** external testers can install and complete the supported flow from documentation; each platform's unverified limits are clearly stated.

### Stage 6 — real user feedback

Recruit a small opt-in test group across local-model users, developers, and remote-provider users. Track first-run success, task completion, failure classes, time to first useful result, and qualitative feedback. Do not upload project content or transcripts for analytics; any usage telemetry must be transparent and opt-in.

**Gate:** prioritize changes using observed failures and user feedback, with privacy-preserving measurement.

### Stage 7 — audience and project operations

Maintain a clear GitHub README, screenshots, release page, issue templates, short real workflow demos, and a feedback path. Demonstrations must identify which steps were actually executed.

### Stage 8 — expansion and business model

Only after the core release gates hold, evaluate extensions, automation, team features, optional paid features, or hosted services. Preserve a useful free entry path and prefer users' own keys for paid providers until operating costs and demand are understood.

## Priority policy

- **P0:** stable builds/releases, projects/files, model execution, persistence, errors, safe agent access, diff/approval/undo, critical scenario verification, documentation.
- **P1:** onboarding, task history, diagnostics, verification reports, provider reliability, privacy and cost clarity.
- **P2:** extension ecosystem, advanced routing, teams, business automation, monetization.

## Engineering and release rules

1. Inspect the existing implementation before adding a parallel subsystem.
2. For each significant change, run relevant automated checks and the real user scenario when available.
3. Keep source/build success, artifact integrity, publication, installation, and runtime proof as separate claims.
4. Record exact version, commit, artifact hashes, checks, known failures, and device/environment coverage.
5. Keep user data and workspaces intact during testing; use isolated fixtures for agent execution.
6. Do not declare product readiness while any required P0 scenario lacks evidence.

## Architecture direction

Continue with Tauri, Rust, and React/TypeScript while that keeps boundaries testable. Maintain clear responsibilities for desktop shell, workspace access, provider adapters, agent runtime, tools/permissions, task state, verification, persistence, diagnostics, and updates. Split modules when it improves testability or safety, not just to match a diagram.

## First work sequence

1. Close and document remaining integration audit findings.
2. Prove tool-call approval/rejection and workspace isolation with the pinned OpenHands version.
3. Complete desktop file/editor and model-switch scenarios in a built/installed app.
4. Complete Android 15+ install and remote task scenarios.
5. Implement diff/undo and task cancellation after their source/runtime contracts are established.
6. Repair CI Android release workflow, publish a verifiable candidate, and perform clean install/upgrade checks.
7. Continue through user feedback and only then evaluate P2 expansion.
