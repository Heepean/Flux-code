# Implementation status

This file tracks the path to a dependable daily-use release. “Implemented” describes code presence; it does not imply runtime verification.

## P0 — release-blocking foundation

| Requirement | Status | Evidence / next check |
| --- | --- | --- |
| Reproducible frontend build | Verified in current audit | `npm run build` passed 2026-10-12 |
| Windows installers | Official beta.3 installers published; install test pending | Signed updater NSIS/MSI assets exist; install and smoke test on clean Windows |
| Android APK | Built and package metadata/signature checked; runtime unverified | v1.0.13 ARM64 debug APK; install and smoke test on Android 15+ device |
| Open project and edit/save files | Implemented; runtime verification pending | Run scenario on clean supported Windows; test read-only, large, external-change and failed-save cases |
| Create/switch projects | Implemented; native path helper covered | Rust test validates child-folder creation and rejects traversal; native folder-picker and Tauri UI flow still need runtime verification |
| Multiple chats per project | Implemented; runtime verification pending | UI stores independent transcripts and OpenHands IDs by workspace; exercise chat switching, reload restore and continuation in desktop app |
| Local GGUF model flow | Partially verified through dev app | Tauri native command loaded temporary 1B GGUF, health/chat passed, stop killed server; user 8B model refused due RAM. Verify selected full-size model on suitable hardware |
| OpenHands model/task flow | Partially verified end to end | Dev app approval/continuation scenarios were previously exercised. Direct OpenHands 1.26.0 probe verified `openai/kilo-auto/free` returned a real answer and persisted `finished`; tool actions and other providers remain in scope |
| Agent action approval | Verified for file_editor create action | Against OpenHands 1.26.0 + Kilo Free in disposable workspace: action displayed before decision; reject left file absent; approve created exact requested file; continuation after server restart succeeded. Runtime workspace escape and other tool types remain unverified |
| Workspace access boundary | Implemented for native commands; incomplete for runtime | Native selected root, rootless IPC for file/search/Git/model download, canonical target checks; OpenHands file_editor scope outside workspace and terminal sandbox are unverified |
| Change diff and real undo | Missing | Implement captured before-state/diff and rollback |
| Error diagnostics and process cleanup | Partial | Windows process-tree regression test confirms a wrapper stop kills its descendant; test real app startup failure, cancellation and repeated launch |
| Data/history persistence | Partial | Project/chat/transcript/conversation IDs now persist in local storage; desktop reload/switch behavior and durable activity/approval journal need runtime verification |
| Public release | Beta candidate active; Android asset pending | Beta.3 signed Windows assets published; Android job failed at SDK setup. Separate license/package workflow steps are staged for next candidate |

## P1 — usability and reliability

- First-run setup and readiness check.
- Session/task list, resumable transcript and action journal.
- Editor dirty-state, tabs/session restore, external-change handling and diff.
- Unified provider configuration with request timeout, cancellation and bounded retries.
- Privacy/data destination and paid-provider cost notices at the point of use.
- Repeatable integration test suite including offline and failure cases.
- Investigate non-fatal OpenHands 1.26.0 browser-tool preload error (`Server.list_tools` missing); the agent server still becomes ready and text tasks succeed.
- Clean-system installer and updater validation.

## P2 — expansion after P0/P1

- Heuristic model router, extensions/integrations, advanced automation, team features and optional commercial services.

## Release rule

Never mark a row verified from source inspection alone. Each release records exact commit/tag, artifact hashes, build/test results, install smoke tests, known failures, and publication links.
