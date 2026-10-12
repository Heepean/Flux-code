# Flux Code / Flux Hands project audit

Audit snapshot: 2026-10-12, local checkout `D:\Flux code\repo-check`, changes prepared for `v1.0.13-beta.1` based on `e73a795` (`v1.0.12`). Flux Code and Flux Hands refer to this one project. This is a source/build audit, not a claim that every workflow was exercised on a clean user machine.

## Architecture observed

- Frontend: React 18, TypeScript, Vite, Monaco Editor, Lucide icons. The primary UI and state live in `src/main.tsx`; styling is in `src/style.css`.
- Desktop/mobile shell: Tauri 2. Rust commands in `src-tauri/src/lib.rs` provide directory/file operations, local model process management, Hugging Face downloads, project search and Git status.
- Agent integration: `src-tauri/src/openhands.rs` starts or connects to OpenHands Agent Server, manages credentials, configures provider/model, sends messages and polls REST events for streamed UI updates.
- Local inference: Rust launches one `llama-server` child at loopback port 8080. Starting another model kills and waits for the current child. The UI uses the same endpoint for the local provider.
- Android: same Tauri UI with a separate remote-server connection flow. Local GGUF selection/download is desktop-only; Android project/tools run on the remote server.
- Persistence: workspace/provider/model settings and active conversation ID are stored in browser `localStorage`; provider keys are delegated to platform credential storage. Conversation transcript is not restored by the UI after restart. OpenHands runtime data is placed under the app data directory.
- Distribution: GitHub Actions builds signed Windows updater assets on tag push, then an Android ARM64 debug APK. The updater checks after startup and every 24 hours, deferring during an agent task/model startup.

No embedded VS Code or OpenHands source tree is present in this checkout. The app is a smaller Tauri product using Monaco and the OpenHands server API.

## Feature evidence and status

| Capability | Source status | Verification in this audit |
| --- | --- | --- |
| React/Tauri desktop app and Monaco editor | Implemented | `npm run build` passed |
| Browse project, list/read/write files | Implemented | Source reviewed; no clean Windows runtime scenario run here |
| Save editor content | Implemented as immediate write on editor change | Source reviewed; no external-change/conflict test; no dirty-state/recovery flow evident |
| Workspace text search | Implemented in Rust; skips selected directories and large/binary files | Rust unit tests pass (8 total) |
| Git branch/status and changed file opening | Implemented (read-only status) | Rust temp-repository test passes |
| Local GGUF launch/switch | Implemented with CPU runtime bootstrap, RAM check and one child process | Source reviewed; no real model execution in this audit |
| Hugging Face model search/download | Implemented for desktop | Source reviewed; no network/download scenario run here |
| OpenHands local/remote task execution | Implemented using API/event polling | Source reviewed; no real Agent Server/provider end-to-end scenario in this audit |
| Kilo free catalog | Implemented against live Kilo endpoint | Source reviewed; no live-provider task run here |
| Android remote agent flow | Implemented | 1.0.13 ARM64 debug APK built; signature and package metadata checked; no phone/emulator attached, so not installed/launched |
| Auto-update | Configured for GitHub Releases, signed updater manifest expected | Not exercised against a published release; local Windows installers lack updater signatures |
| Change diff, undo/rollback, terminal panel, risky-action approval | Not implemented in UI | Confirmed by source review; see critical issues |
| Android user experience | Remote-only flow implemented | APK is ARM64 debug-signed, not Play Store/release signed |

## Critical findings and risks

### P0 — Agent action confirmation needs real runtime proof

The source originally sent `NeverConfirm`. This turn changed new conversations to `AlwaysConfirm`, updates reused conversations through the Agent Server confirmation-policy endpoint, polls pending ActionEvents, and presents their tool/action details with explicit approve/reject controls. Rejecting an action reports that the OpenHands run stopped (the pinned API transitions the conversation to idle). Rust tests and frontend build pass, but an end-to-end test with OpenHands 1.26 has not yet proven that no tool runs before approval or that approval/rejection works in a live run. Diff and undo are still absent. Do not claim the action safety flow verified until the runtime scenario passes.

### P0 — Workspace boundary only covers Flux Hands file commands

Desktop native state records the active root from the OS folder picker and persists it; workspace file/list/search/Git/model-download commands no longer accept a caller-supplied root. Targets are canonicalized and rejected outside that active root, and editor saves only affect existing files. Unit coverage verifies that a sibling path and a symlink escape are rejected where symlinks are supported. Windows junction escape and root-switch races still need coverage. This does not sandbox OpenHands' independent runtime or terminal tool; test and document that boundary separately.

### P0 — GitHub stable release lags current candidate

GitHub `main` is at `e73a795`; latest stable public release is v1.0.8 with Windows assets and no Android APK. Local candidate changes are uncommitted and not published. Publishing v1.0.13-beta.1 requires a Git push and tag-triggered Actions; publication must be confirmed by inspecting the public release assets.

### P1 — UI state persistence is incomplete

Workspace settings and conversation ID survive reload, while in-memory messages do not. The user can resume the server conversation ID without seeing a locally restored transcript. Storage migration and damaged-data handling are not evident.

### P1 — Editor is intentionally minimal

Monaco is present, but there is no visible dirty-state tracking, tab/session restore, save conflict detection for external edits, navigation/search integration in the editor, diff view, or undo at agent-action level. Current save writes as the editor changes.

### P1 — Task lifecycle and cancellation are incomplete

The UI has one busy flag and streams event text/status, but no task list, pause/cancel action, structured approval state, or durable activity journal was found. Agent polling and server process behavior need end-to-end cancellation/exit testing.

### P1 — Release maturity differs by platform

The GitHub workflow is designed to create signed Windows updater assets and an ARM64 debug APK. Local Windows NSIS/MSI builds succeeded with updater artifact generation disabled because signing secrets are unavailable; NSIS is not Authenticode-signed. Local APK is debug-signed. Neither artifact has been installed and smoke-tested on a clean Windows PC or Android device in this audit.

### P1 — Product says “no account/no limits” only within provider constraints

Local inference needs hardware/runtime; Kilo's anonymous free tier has upstream rate limits; remote providers can charge and retain prompts. Existing README discloses these constraints. Maintain this accuracy in UI and release copy.

## Build and repository state

- `npm run build`: passed on 2026-10-12 (TypeScript compile and Vite production build).
- `cargo test --manifest-path src-tauri/Cargo.toml`: passed 8 tests on 2026-10-12 after workspace-boundary work.
- After the confirmation flow change: `cargo test --manifest-path src-tauri/Cargo.toml` passed (8 tests) and `npm run build` passed on 2026-10-12.
- Windows NSIS (3,815,192 bytes) and MSI (5,677,056 bytes) v1.0.13 installers built locally. Tauri updater signing failed because the private signing key is only available in GitHub Actions; no local Authenticode signature was established.
- Android ARM64 debug APK built locally at `src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk` (SHA256 `E2AC4B5C84AA7023C801B8235D1D7D46E1B480A289361137653AF9C9265E2A9B`); apksigner v2 verification and package/version metadata passed. No Android device/emulator was attached.
- GitHub latest stable release verified through public API: v1.0.8, Windows NSIS/MSI and updater assets present, Android APK absent. Candidate publication is pending.
- Clean-machine installed-app smoke test: not performed.

## Immediate implementation order

1. Exercise approval and rejection with a live OpenHands 1.26 test server; verify that no action runs before approval and rejected runs visibly stop.
2. Add Windows junction/root-switch race coverage to the selected-workspace boundary; establish and test the OpenHands runtime's actual containment.
3. Add task stop/cancel behavior and durable event/transcript state; test process cleanup on app exit and failure.
4. Implement agent change preview (diff) and rollback based on recorded file state.
5. Run repeatable integration scenarios for llama.cpp, local/remote OpenHands, provider switching, and Android remote connection.
6. Complete editor/session persistence and external file change handling.
7. Publish a beta candidate through CI, inspect all assets, and install-test on Windows and Android before any stable readiness claim.

## Scope limits

This audit reviewed the available source, repository metadata, release workflow, and local build artifacts. It did not run an OpenHands server with a real model, install the desktop application on a clean machine, connect an Android device, conduct a security penetration test, or publish to GitHub. Statuses must be updated when those checks are actually carried out.
