# Flux Code / Flux Hands — master roadmap

Flux Code and Flux Hands are names for the same product. This roadmap describes the direction and release gates; the [project audit](PROJECT_AUDIT.md) and [implementation status](IMPLEMENTATION_STATUS.md) record what is actually present and verified. Code presence alone never counts as runtime proof.

## Product goal

Build a dependable Windows desktop workspace for projects, local GGUF models, and remote model providers. OpenHands Agent Server is required only for local GGUF agent tasks on desktop; remote providers use direct chat without requiring it. Android is a remote-only companion for direct model chat and does not run Agent Server. The product should make the chosen model and data destination clear.

## Product principles

- **Simple:** a first-time user can open a project and select a model without developer setup instructions.
- **Flexible:** local models and compatible remote providers share a coherent configuration flow. Free catalogs are refreshed from their provider and described with provider limits.
- **Useful:** agents use real project tools and report changed files and actual checks.
- **Trustworthy:** the user can see workspace scope, external data destinations, risky actions, and test results. Never claim an action succeeded without evidence.
- **Incremental:** stabilize the desktop product and mobile remote flow before building commercial features or a general extension platform.

## Current verified baseline (2026-10-13)

- On 2026-10-13, `npm run build`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and all 33 Rust tests passed. The Android ARM64 Rust target check also passed with NDK 27.2.12479018.
- Both supplied user GGUFs completed local inference on CPU and GPU using disposable llama-server builds. Nemotron Q6_K reached about 57 tokens/s on the RX 7700 XT Vulkan backend; Ternary Bonsai PQ2_0 reached about 40 tokens/s with PrismML. The 27B model works on CPU too but leaves about 0.34 GB free RAM and runs at about 4 tokens/s, so Vulkan GPU is the practical mode on this PC.
- Added a persisted CPU/Vulkan choice and format-aware runtime bootstrap. The desktop runtime installer itself still needs a clean native Tauri invocation test before calling the feature verified end-to-end.
- A regression test now proves a running llama.cpp process is stopped before the next model's RAM preflight, preventing the previous model's resident memory from causing a false refusal. Direct full-size model inference was verified, while the app-mediated model switching flow is still pending native UI validation.
- Windows file-boundary tests create an NTFS junction from inside the project to an outside folder and verify the escaped path is rejected. Workspace root mutation is serialized in the UI and behind a native Rust guard during OpenHands requests; the native desktop live race scenario remains unverified.
- The app started OpenHands Agent Server 1.26.0 in a disposable workspace and completed a Kilo Free text task, including streamed UI events and clean server shutdown. A live `file_editor.create` action was displayed before execution: rejection kept the file absent, approval created the requested file, and the same conversation resumed after server restart.
- That OpenHands startup logged a non-fatal browser-tool preload error (`Server.list_tools` missing); text tasks worked. A prior live dev-app scenario verified file-editor approval/rejection and conversation continuation; runtime workspace containment remains unverified.
- A direct pinned OpenHands 1.26.0 request using the app's `openai/kilo-auto/free` model ID returned the expected Kilo response and finished; the full native UI flow still needs a desktop-window smoke test.
- Project creation and per-project chats/transcripts are implemented; browser preview showed the open-existing/create-new dialog and new-chat entry. The selected chat ID is now persisted separately per workspace so restart restoration can reopen the user's last chat; native folder dialogs, project switching and chat reload/continuation still need UI runtime verification.
- The editor now has explicit Save/Ctrl+S and dirty indicators; only the production frontend build has been verified for this change. Native editing, failed save and external-change scenarios remain pending.
- GitHub publishes `v1.0.16-beta.2` with a signed Windows setup/MSI/updater set and Android ARM64 debug APK; `v1.0.8` remains latest stable. The hosted release gate passed and all five expected assets return HTTP 200. Windows clean install/upgrade and Android installation/runtime remain unverified. See [the current release](https://github.com/Heepean/Flux-code/releases/tag/v1.0.16-beta.2) and [implementation status](IMPLEMENTATION_STATUS.md).

See [PROJECT_AUDIT.md](PROJECT_AUDIT.md) for evidence and limitations, and [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for release gates.

## Staged plan

### Stage 0 — verified audit

Keep an evidence-backed feature matrix, architecture notes, reproducible build instructions, defect register, and environment limitations. Re-audit any claim when the runtime or release workflow changes.

**Gate:** major features have a status, reproducible scenario, severity, and next verification step.

### Stage 1 — dependable base product (P0)

- Verify first launch, folder selection, file listing/open/edit/save, errors, and restart persistence on Windows.
- Fix critical local model and OpenHands integration defects; test startup, model switch, request, failure, cancellation, and shutdown.
- Verify direct remote model chat on Android 15+ hardware or emulator; Android does not use Agent Server.
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
2. Local Nemotron/OpenHands 1.26.0 at 16K context now has a verified approve/execute/observation cycle in an isolated workspace. Next, repeat the same flow through the native Tauri model-selection and approval UI, then verify workspace containment for terminal and file-editor actions.
3. Complete desktop file/editor and model-switch scenarios in a built/installed app.
4. Complete Android 15+ install and remote task scenarios.
5. Implement diff/undo and task cancellation after their source/runtime contracts are established.
6. CI release publication is verified for v1.0.16-beta.2; perform clean Windows install/upgrade checks and Android 15+ installation plus remote-chat smoke test.
7. Continue through user feedback and only then evaluate P2 expansion.


- v1.0.16 routes Kilo and OpenAI-compatible remote chat directly to the provider on desktop and Android; OpenHands Agent Server remains attached to desktop local-model tasks. The uv archive extractor locates assets by ZIP index and verifies upstream digests. Frontend build and all 33 Rust tests pass. Android cross-check and ARM64 APK assembly passed locally after copying the compiled library around the Windows symlink restriction; the published APK also passed CI signature verification. No Android device is attached, so installation/runtime remain unverified.
