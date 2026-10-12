# Implementation status

This file tracks the path to a dependable daily-use release. “Implemented” describes code presence; it does not imply runtime verification.

## P0 — release-blocking foundation

| Requirement | Status | Evidence / next check |
| --- | --- | --- |
| Reproducible frontend build | Verified in current audit | `npm run build` passed 2026-10-12 |
| Windows installers | Stable v1.0.8 release page reports 6 assets; candidate beta.2 workflow is in progress | Verify exact installer/updater names, signatures and hashes after Actions run 38220704939; install and smoke test on clean Windows |
| Android APK | No current candidate APK verified | Candidate beta.2 Android job runs after Windows release job; previous Android SDK setup failed. Verify APK signature, package metadata, hash, then install on Android 15+ |
| Open project and edit/save files | Implemented with explicit Save/Ctrl+S and dirty-tab indicator; build verified | Native desktop save, failed-save recovery, large/read-only files and external-change conflicts still need runtime checks |
| Git change review | Per-file diff preview added for tracked and untracked text files | Rust tests cover staged, unstaged, untracked, spaced paths, and rejecting files outside the selected workspace; native panel remains visually unverified |
| Create/switch projects | Implemented; Rust helper and browser-preview dialog checked | Rust test validates child-folder creation and rejects traversal; folder picker and project switching in native Tauri UI still need runtime verification |
| Multiple chats per project | Implemented; creation visible in browser preview | UI stores independent transcripts and OpenHands IDs by workspace; desktop switching, reload restore and continuation remain unverified |
| Local GGUF model flow | Partially verified through dev app | Tauri native command loaded temporary 1B GGUF, health/chat passed, stop killed server; user 8B model refused due RAM. Verify selected full-size model on suitable hardware |
| OpenHands model/task flow | Partially verified end to end | Dev app approval/continuation scenarios were previously exercised. Direct OpenHands 1.26.0 probe verified `openai/kilo-auto/free` returned a real answer and persisted `finished`; tool actions and other providers remain in scope |
| Agent action approval | Verified for file_editor create action | Against OpenHands 1.26.0 + Kilo Free in disposable workspace: action displayed before decision; reject left file absent; approve created exact requested file; continuation after server restart succeeded. Runtime workspace escape and other tool types remain unverified |
| Workspace access boundary | Implemented for native commands; incomplete for runtime | Native selected root, rootless IPC for file/search/Git/model download, canonical target checks; OpenHands file_editor scope outside workspace and terminal sandbox are unverified |
| Change diff and agent file-editor undo | Git diff plus guarded rollback for successful OpenHands file_editor changes implemented and unit-tested | Rollback checks workspace containment and current content against the recorded after-state; terminal effects, deletions/binary changes, and remote workspace rollback are not covered. File-editor history is persisted with each local chat transcript |
| Error diagnostics and process cleanup | Partial | Windows process-tree regression test confirms wrapper cleanup; OpenHands interrupt endpoint/header pass HTTP mock coverage; live-agent cancellation and repeated launch remain unverified |
| Data/history persistence | Partial | Project/chat/transcript/conversation IDs now persist in local storage; desktop reload/switch behavior and durable activity/approval journal need runtime verification |
| Public release | v1.0.14-beta.2 workflow remains active without artifacts; latest public release is v1.0.8 | Actions run 38220704939 still reports In progress and no artifacts; no new installer/APK verified. beta.2 does not include later commits bfafbac, 90eaadd or f0fb1ad |

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
