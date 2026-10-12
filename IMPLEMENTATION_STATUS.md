# Implementation status

This file tracks the path to a dependable daily-use release. “Implemented” describes code presence; it does not imply runtime verification.

## P0 — release-blocking foundation

| Requirement | Status | Evidence / next check |
| --- | --- | --- |
| Reproducible frontend build | Verified in current audit | `npm run build` passed 2026-10-12 |
| Windows installers | Built locally, not install-tested | v1.0.13 NSIS/MSI built; no local updater signature or Authenticode signature; use CI artifacts for official release |
| Android APK | Built and package metadata/signature checked; runtime unverified | v1.0.13 ARM64 debug APK; install and smoke test on Android 15+ device |
| Open project and edit/save files | Implemented; runtime verification pending | Run scenario on clean supported Windows; test read-only, large, external-change and failed-save cases |
| OpenHands model/task flow | Implemented; end-to-end verification pending | Test local runtime and remote server with tool-capable model |
| Agent action approval | Implemented in code; runtime proof pending | `AlwaysConfirm`, pending-action display and approve/reject endpoint; test against OpenHands 1.26 and verify fail-closed behavior |
| Workspace access boundary | Implemented for native commands; incomplete for runtime | Native selected root, rootless IPC for file/search/Git/model download, canonical target checks; cover Windows junction/race cases and OpenHands tool sandbox separately |
| Change diff and real undo | Missing | Implement captured before-state/diff and rollback |
| Error diagnostics and process cleanup | Partial | Some errors shown and child processes stopped on exit; test crashes, startup timeout, cancellation and repeated launch |
| Data/history persistence | Partial | Preferences and conversation ID persist; transcript/activity not restored in UI |
| Public release | Pending candidate publication | GitHub stable latest is v1.0.8 and has no APK; CI release workflow requires signing secrets, candidate changes are not yet pushed |

## P1 — usability and reliability

- First-run setup and readiness check.
- Session/task list, resumable transcript and action journal.
- Editor dirty-state, tabs/session restore, external-change handling and diff.
- Unified provider configuration with request timeout, cancellation and bounded retries.
- Privacy/data destination and paid-provider cost notices at the point of use.
- Repeatable integration test suite including offline and failure cases.
- Clean-system installer and updater validation.

## P2 — expansion after P0/P1

- Heuristic model router, extensions/integrations, advanced automation, team features and optional commercial services.

## Release rule

Never mark a row verified from source inspection alone. Each release records exact commit/tag, artifact hashes, build/test results, install smoke tests, known failures, and publication links.
