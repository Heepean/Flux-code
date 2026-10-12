# Implementation status

This file tracks the path to a dependable daily-use release. “Implemented” describes code presence; it does not imply runtime verification.

## P0 — release-blocking foundation

| Requirement | Status | Evidence / next check |
| --- | --- | --- |
| Reproducible frontend build | Verified in current audit | `npm run build` passed 2026-10-12 |
| Windows installers | Latest public release remains v1.0.8; its API lists four Windows/updater assets | Clean Windows install and upgrade remain unverified. v1.0.15-beta.1 built and uploaded Windows files, but the validation expected `Flux Hands_...msi` instead of the actual `Flux.Hands_...msi`; corrected locally. Android package ID was also corrected; rerun pending |
| Android APK | Optimized v1.0.15-beta.1 ARM64 candidate rebuilt from local commit `d5e2465` and signed for testing; no APK is attached to public v1.0.8 | `artifacts/Flux.Hands_1.0.15-beta.1_android-arm64.apk`, 17,150,827 bytes; package `dev.fluxcode.fluxhands`, versionCode 1001015, minSdk 35, targetSdk 37; APK signature and zip alignment verified; SHA-256 `134956B97587DAF3DDCCB4E5B339E9E1A1DB3849BF88E68763E40F0055E3A370`. Signed with this machine's Android debug key; no device/emulator attached |
| Open project and edit/save files | Implemented with explicit Save/Ctrl+S and dirty-tab indicator; build verified | Native desktop save, failed-save recovery, large/read-only files and external-change conflicts still need runtime checks |
| Git change review | Per-file diff preview added for tracked and untracked text files | Rust tests cover staged, unstaged, untracked, spaced paths, and rejecting files outside the selected workspace; native panel remains visually unverified |
| Create/switch projects | Implemented; picker and folder creation return paths, while a serialized frontend switch performs activation | Rust tests cover child-folder creation, traversal rejection and Windows junction escape. Production build passes; actual project switching during a live task still needs desktop-window verification |
| Multiple chats per project | Implemented; creation visible in browser preview | UI stores independent transcripts and OpenHands IDs by workspace; desktop switching, reload restore and continuation remain unverified |
| Local GGUF model flow | Partially verified through dev app | Tauri native command loaded temporary 1B GGUF, health/chat passed, stop killed server; user 8B model refused due RAM. Verify selected full-size model on suitable hardware |
| OpenHands model/task flow | Partially verified end to end | Dev app approval/continuation scenarios were previously exercised. Direct OpenHands 1.26.0 probe verified `openai/kilo-auto/free` returned a real answer and persisted `finished`; tool actions and other providers remain in scope |
| Agent action approval | Verified for file_editor create action | Against OpenHands 1.26.0 + Kilo Free in disposable workspace: action displayed before decision; reject left file absent; approve created exact requested file; continuation after server restart succeeded. Runtime workspace escape and other tool types remain unverified |
| Workspace access boundary | Native path checks cover symlinks and Windows junctions; backend serializes root changes with agent requests | Windows junction and agent-workspace guard tests pass; a local agent request also checks its configured workspace against the active root. OpenHands file_editor/terminal remain outside an OS sandbox |
| Change diff and agent file-editor undo | Git diff plus guarded rollback for successful OpenHands file_editor changes implemented and unit-tested | Rollback checks workspace containment and current content against the recorded after-state; terminal effects, deletions/binary changes, and remote workspace rollback are not covered. File-editor history is persisted with each local chat transcript |
| Error diagnostics and process cleanup | Partial; live OpenHands interrupt route now verified | In a disposable OpenHands 1.26.0 server with Kilo Free, an approved 45-second terminal wait was interrupted; HTTP 200, `InterruptEvent`, `paused` status, interrupted-tool error, and exited Python process were observed. Native Tauri button-to-event flow and repeated app launch/cleanup still need verification |
| Data/history persistence | Partial | Project/chat/transcript/conversation IDs now persist in local storage; desktop reload/switch behavior and durable activity/approval journal need runtime verification |
| Public release | Latest public release remains v1.0.8; v1.0.15-beta.1 is an unpublished draft workflow run | v1.0.8 has no APK because Android setup failed. The new workflow correctly withheld publication: its Android SDK package ID and Windows MSI filename checks both needed correction. Both are fixed locally; rerun and final asset verification remain pending |

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
