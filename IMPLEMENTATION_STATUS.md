# Implementation status

This file tracks the path to a dependable daily-use release. “Implemented” describes code presence; it does not imply runtime verification.

## P0 — release-blocking foundation

| Requirement | Status | Evidence / next check |
| --- | --- | --- |
| Reproducible frontend build | Verified in current audit | `npm run build` passed 2026-10-12 |
| Windows installers | v1.0.15-beta.1 is now public as a prerelease; v1.0.8 remains latest stable | Public release contains the setup EXE, updater signature, MSI, and `latest.json`; public download entries and SHA-256 digests verified. Clean install/upgrade remain unverified |
| Android APK | v1.0.15-beta.1 ARM64 debug APK is public for Android 15+ testing | GitHub Release asset `Flux.Hands_v1.0.15-beta.1_android-arm64-debug.apk`, 179,624,315 bytes; public asset SHA-256 `aa082df4ea197a24a47d02ba673b66c1f3e8a1880e5844e949c63e503235529c`; hosted `apksigner verify` passed and public HEAD returned HTTP 200. A separate optimized 17,150,827-byte local candidate is at `artifacts/Flux.Hands_1.0.15-beta.1_android-arm64.apk`. No device/emulator attached, so installation/runtime remain unverified |
| Open project and edit/save files | Implemented with explicit Save/Ctrl+S and dirty-tab indicator; build verified | Native desktop save, failed-save recovery, large/read-only files and external-change conflicts still need runtime checks |
| Git change review | Per-file diff preview added for tracked and untracked text files | Rust tests cover staged, unstaged, untracked, spaced paths, and rejecting files outside the selected workspace; native panel remains visually unverified |
| Create/switch projects | Implemented; picker and folder creation return paths, while a serialized frontend switch performs activation | Rust tests cover child-folder creation, traversal rejection and Windows junction escape. Production build passes; actual project switching during a live task still needs desktop-window verification |
| Multiple chats per project | Implemented; local browser preview verified | Created a new chat and confirmed it remained listed after page reload. UI stores independent transcripts and OpenHands IDs by workspace; desktop switching, populated transcript restore and continuation still need a native-window scenario |
| Local GGUF model flow | Partially verified through dev app | Tauri native command loaded temporary 1B GGUF, health/chat passed, stop killed server; user 8B model refused due RAM. Verify selected full-size model on suitable hardware |
| OpenHands model/task flow | Partially verified end to end | Dev app approval/continuation scenarios were previously exercised. Direct OpenHands 1.26.0 probe verified `openai/kilo-auto/free` returned a real answer and persisted `finished`; tool actions and other providers remain in scope |
| Agent action approval | Verified for file_editor create action | Against OpenHands 1.26.0 + Kilo Free in disposable workspace: action displayed before decision; reject left file absent; approve created exact requested file; continuation after server restart succeeded. Runtime workspace escape and other tool types remain unverified |
| Workspace access boundary | Native path checks cover symlinks and Windows junctions; backend serializes root changes with agent requests | Windows junction and agent-workspace guard tests pass; a local agent request also checks its configured workspace against the active root. OpenHands file_editor/terminal remain outside an OS sandbox |
| Change diff and agent file-editor undo | Git diff plus guarded rollback for successful OpenHands file_editor changes implemented and unit-tested | Rollback checks workspace containment and current content against the recorded after-state; terminal effects, deletions/binary changes, and remote workspace rollback are not covered. File-editor history is persisted with each local chat transcript |
| Error diagnostics and process cleanup | Partial; live OpenHands interrupt route now verified | In a disposable OpenHands 1.26.0 server with Kilo Free, an approved 45-second terminal wait was interrupted; HTTP 200, `InterruptEvent`, `paused` status, interrupted-tool error, and exited Python process were observed. Native Tauri button-to-event flow and repeated app launch/cleanup still need verification |
| Data/history persistence | Partial | Project/chat/transcript/conversation IDs now persist in local storage; desktop reload/switch behavior and durable activity/approval journal need runtime verification |
| Public release | v1.0.15-beta.1 is published as a prerelease; v1.0.8 remains latest stable | Run 38231141807 published the APK and Windows/updater assets after the five-asset gate passed. It then failed because `publish_release` had not checked out the script used for stale-asset cleanup. Added checkout locally for future tag runs; older-release cleanup and a green rerun remain outstanding |

## P1 — usability and reliability

- First-run setup and readiness check.
- Session/task list, resumable transcript and action journal.
- Editor dirty-state, tabs/session restore, external-change handling and diff.
- Unified provider configuration with bounded retries. OpenHands interrupt API and stop button are implemented and mock-tested; live cancellation still needs verification.
- Privacy/data destination and paid-provider cost notices at the point of use.
- Repeatable integration test suite including offline and failure cases.
- Investigate non-fatal OpenHands 1.26.0 browser-tool preload error (`Server.list_tools` missing); the agent server still becomes ready and text tasks succeed.
- Clean-system installer and updater validation.

## P2 — expansion after P0/P1

- Heuristic model router, extensions/integrations, advanced automation, team features and optional commercial services.

## Release rule

Never mark a row verified from source inspection alone. Each release records exact commit/tag, artifact hashes, build/test results, install smoke tests, known failures, and publication links.
