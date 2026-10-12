# Flux Code / Flux Hands project audit

Audit snapshot: 2026-10-12, local checkout `D:\Flux code\repo-check`, preparing `v1.0.14-beta.1` after `v1.0.13-beta.3`. Flux Code and Flux Hands refer to this one project. This is a source/build and focused runtime audit, not a claim that every workflow was exercised on a clean user machine.

## Architecture observed

- Frontend: React 18, TypeScript, Vite, Monaco Editor, Lucide icons. The primary UI and state live in `src/main.tsx`; styling is in `src/style.css`.
- Desktop/mobile shell: Tauri 2. Rust commands in `src-tauri/src/lib.rs` provide directory/file operations, local model process management, Hugging Face downloads, project search and Git status.
- Agent integration: `src-tauri/src/openhands.rs` starts or connects to OpenHands Agent Server, manages credentials, configures provider/model, sends messages and polls REST events for streamed UI updates.
- Local inference: Rust launches one `llama-server` child at loopback port 8080. Starting another model kills and waits for the current child. The UI uses the same endpoint for the local provider.
- Android: same Tauri UI with a separate remote-server connection flow. Local GGUF selection/download is desktop-only; Android project/tools run on the remote server.
- Persistence: workspace/provider/model settings, project list, per-project chats, chat transcripts and each chat's OpenHands conversation ID are stored in browser `localStorage`; provider keys are delegated to platform credential storage. OpenHands runtime data is placed under the app data directory.
- Distribution: GitHub Actions builds signed Windows updater assets on tag push, then an Android ARM64 debug APK. The updater checks after startup and every 24 hours, deferring during an agent task/model startup.

No embedded VS Code or OpenHands source tree is present in this checkout. The app is a smaller Tauri product using Monaco and the OpenHands server API.

## Feature evidence and status

| Capability | Source status | Verification in this audit |
| --- | --- | --- |
| React/Tauri desktop app and Monaco editor | Implemented | `npm run build` passed |
| Browse project, list/read/write files | Implemented | Source reviewed; no clean Windows runtime scenario run here |
| Create and switch projects | Implemented in UI and native commands | `cargo test` covers safe child-folder creation and traversal rejection; build and Rust checks pass. Native folder-picker and full Tauri project-switch flow are not yet exercised |
| Multiple chats per project | Implemented with transcript and OpenHands conversation-ID persistence | Source/build verified; switching, reload restoration and independent server continuation have not yet been exercised in the desktop UI |
| Save editor content | Implemented as immediate write on editor change | Source reviewed; no external-change/conflict test; no dirty-state/recovery flow evident |
| Workspace text search | Implemented in Rust; skips selected directories and large/binary files | Rust unit tests pass (9 total) |
| Git branch/status and changed file opening | Implemented (read-only status) | Rust temp-repository test passes |
| Local GGUF launch/switch | Implemented with CPU runtime bootstrap, RAM check and one child process | Real small GGUF loaded through native app commands; health and chat passed, then stop closed the port. Full user models are refused on this machine's available RAM |
| Hugging Face model search/download | Implemented for desktop | Source reviewed; no network/download scenario run here |
| OpenHands local/remote task execution | Implemented using API/event polling | Earlier dev-app E2E covered Kilo Free events and approval. A separate live OpenHands 1.26.0 run with `openai/kilo-auto/free` returned `KILO_AUTO_PROBE_OK`; persisted conversation state reached `finished` |
| Kilo free catalog | Implemented against live Kilo endpoint | Anonymous Kilo Auto Free request through OpenHands 1.26.0 returned the expected marker with zero reported model cost; tool-capable action flow and provider limits remain in scope |
| Android remote agent flow | Implemented | 1.0.13 ARM64 debug APK built; signature and package metadata checked; no phone/emulator attached, so not installed/launched |
| Auto-update | Configured for GitHub Releases, signed updater manifest expected | Not exercised against a published release; local Windows installers lack updater signatures |
| Change diff, undo/rollback, terminal panel, risky-action approval | Not implemented in UI | Confirmed by source review; see critical issues |
| Android user experience | Remote-only flow implemented | APK is ARM64 debug-signed, not Play Store/release signed |

## Critical findings and risks

### P0 — Agent action confirmation needs real runtime proof

The source originally sent `NeverConfirm`. New conversations use `AlwaysConfirm`; reused conversations call the Agent Server confirmation-policy endpoint, poll pending ActionEvents and present tool/action details with explicit approve/reject controls. A dev-app E2E test against OpenHands 1.26.0 and Kilo Free verified both outcomes in a disposable workspace: the file did not exist before either decision; rejection left it absent and reported that execution stopped; approval created only the proposed file with the requested bytes. The conversation also resumed after the Agent Server process was restarted and returned `CONVERSATION_CONTINUED_OK`, exercising the camelCase continuation and confirmation command arguments. Server startup logs a non-fatal browser-tool preload error (`AttributeError: 'Server' object has no attribute 'list_tools'`) and then reports ready. Remaining: prove the runtime cannot escape its workspace, test confirmation across other tool classes, and build diff/undo.

### P0 — Workspace boundary only covers Flux Hands file commands

Desktop native state records the active root from the OS folder picker and persists it; workspace file/list/search/Git/model-download commands no longer accept a caller-supplied root. Targets are canonicalized and rejected outside that active root, and editor saves only affect existing files. Unit coverage verifies that a sibling path and a symlink escape are rejected where symlinks are supported. Windows junction escape and root-switch races still need coverage. This does not sandbox OpenHands' independent runtime or terminal tool; test and document that boundary separately.

### P0 — Tauri command argument casing broke agent conversation continuation

`send_agent_message` and `respond_to_agent_confirmation` declared Rust `conversation_id`, but the UI sends Tauri arguments as `conversationId`. That meant the first message path omitted the optional field harmlessly, while continuing an existing conversation and responding to approval could fail argument decoding. The commands now accept the frontend camelCase spelling. A real Agent Server message invocation using `conversationId: null` succeeded after this fix; continuation and approval endpoints still need a dedicated live test.

### P0 — Local model command mismatch fixed; broad provider verification remains

The UI sent camelCase `modelPath` and `gpuLayers` to `start_local_model`; Rust expected snake_case in the nested struct, so UI-triggered GGUF startup failed before spawning the server. `StartModel` now deserializes camelCase and a regression test covers this. The dev app's native command was re-run successfully with a temporary 1B model. Kilo was also verified through the local OpenHands server; other providers and full-size local models remain hardware/provider dependent.

### P1 — UI state persistence needs runtime validation

The UI now stores project lists and per-project chat transcripts plus OpenHands conversation IDs in `localStorage`, and migrates the previous single conversation ID. Reload restoration, large transcripts, browser storage failure and independent continuation per chat have not yet been exercised in the desktop UI. Pending approvals and a durable task/event journal are still missing.

### P1 — Editor is intentionally minimal

Monaco is present, but there is no visible dirty-state tracking, tab/session restore, save conflict detection for external edits, navigation/search integration in the editor, diff view, or undo at agent-action level. Current save writes as the editor changes.

### P1 — Task lifecycle and cancellation are incomplete

The UI has one busy flag and streams event text/status, but no task list, pause/cancel action, structured approval state, or durable activity journal was found. Agent polling and server process behavior need end-to-end cancellation/exit testing.

### P1 — Release maturity differs by platform

The GitHub workflow creates signed Windows updater assets and an ARM64 debug APK. Public beta.3 has signed Windows assets, but its Android job failed in the combined SDK license/package installation step before APK build. Workflow now separates those operations so the next run can identify the failing command. Local Windows NSIS/MSI builds and APK are unsigned/debug-signed respectively; neither has been installed and smoke-tested on a clean Windows PC or Android device.

### P1 — Product says “no account/no limits” only within provider constraints

Local inference needs hardware/runtime; Kilo's anonymous free tier has upstream rate limits; remote providers can charge and retain prompts. Existing README discloses these constraints. Maintain this accuracy in UI and release copy.

## Build and repository state

- `npm run build`: passed on 2026-10-12 (TypeScript compile and Vite production build).
- `cargo test --manifest-path src-tauri/Cargo.toml`: passed 11 tests on 2026-10-12, including Windows child-process-tree termination and project-folder traversal rejection.
- Latest `npm run build`, `cargo check --manifest-path src-tauri/Cargo.toml`, `cargo fmt --check`, and `git diff --check` passed on 2026-10-12.
- `npm run build`: passed after the local model command fix.
- Windows NSIS (3,815,192 bytes) and MSI (5,677,056 bytes) v1.0.13 installers built locally. Tauri updater signing failed because the private signing key is only available in GitHub Actions; no local Authenticode signature was established.
- Android ARM64 debug APK built locally at `src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk` (SHA256 `E2AC4B5C84AA7023C801B8235D1D7D46E1B480A289361137653AF9C9265E2A9B`); apksigner v2 verification and package/version metadata passed. No Android device/emulator was attached.
- llama.cpp CPU b11429 archive SHA-256 matched upstream GitHub attestation; temporary Llama 3.2 1B Q4_K_M model SHA-256 matched Hugging Face metadata. Dev app returned `FLUX_TAURI_OK` through its native local-model flow.
- Nemotron-Cascade-8B-Q6_K was rejected before spawn: about 5.0 GB RAM free vs. about 8.3 GB required.
- Local Agent Server E2E: app launched pinned OpenHands 1.26.0 against a disposable workspace, Kilo Free returned `FLUX_OPENHANDS_OK`, events carried the conversation ID and terminal status, and `stop_agent_server` completed. The original workspace configuration was restored after the test.
- Kilo Auto Free probe: a direct pinned OpenHands 1.26.0 server in a disposable workspace accepted `openai/kilo-auto/free`, returned `KILO_AUTO_PROBE_OK`, wrote `execution_status=finished` to its event store, and was stopped with the listening port closed. This validates the model ID/gateway path, not the full Tauri UI send flow.
- Process-tree regression: a Windows test launched a wrapper and child process, ran `terminate_process_tree`, verified the child PID exited, and passed; the production Agent Server path uses this helper.
- New project/chat UI and persistence are source/build checked but not yet exercised through a running desktop window.
- This OpenHands startup produced a non-fatal browser tool preload error (`Server.list_tools` missing); server reached ready state and the no-tools response succeeded. Browser automation tools may be unavailable until the upstream dependency mismatch is resolved.
- GitHub beta.3 has signed Windows assets. Its Android job failed at SDK setup; the separated license/package steps have not yet run in CI.
- Clean-machine installed-app smoke test: not performed.

## Immediate implementation order

1. Exercise approval and rejection with a live OpenHands 1.26 test server; verify that no action runs before approval and rejected runs visibly stop.
2. Publish a CI run with separated Android license and SDK installation steps; finish APK build and verify release assets.
3. Add Windows junction/root-switch race coverage to the selected-workspace boundary; establish and test the OpenHands runtime's actual containment.
4. Add task stop/cancel behavior and durable event/transcript state; test process cleanup on app exit and failure.
5. Implement agent change preview (diff) and rollback based on recorded file state.
6. Run repeatable integration scenarios for local/remote OpenHands, provider switching, and Android remote connection.
7. Complete editor/session persistence and external file change handling.
8. Install-test official installers on clean Windows and Android targets before any stable readiness claim.

## Scope limits

This audit reviewed source, CI/release status and artifacts, ran a real small-GGUF inference request through Tauri native commands, and exercised OpenHands 1.26.0 through the dev app in earlier approval scenarios and directly for the Kilo Auto Free model-path check. It did not install the official desktop release on a clean Windows PC, connect an Android device, exercise the new project/chat UI in a desktop window, or conduct a security penetration test. Statuses must be updated when those checks are carried out.
