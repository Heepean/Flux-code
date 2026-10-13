# Implementation status

This file tracks the path to a dependable daily-use release. “Implemented” describes code presence; it does not imply runtime verification.

## P0 — release-blocking foundation

| Requirement | Status | Evidence / next check |
| --- | --- | --- |
| Reproducible frontend build | Verified in current audit | `npm run build` passed 2026-10-13 |
| Windows installers | v1.0.16-beta.2 is the current prerelease; v1.0.8 remains latest stable | Public setup EXE (3,924,279 bytes), updater signature, MSI (5,869,568 bytes) and `latest.json` all return HTTP 200. Signed updater artifacts passed the hosted release gate. Clean install/upgrade remain unverified |
| Android APK | v1.0.16-beta.2 ARM64 debug APK is public for Android 15+ testing | `Flux.Hands_v1.0.16-beta.2_android-arm64-debug.apk` (181,324,463 bytes), minSdk 35. Public asset returns HTTP 200; CI and local `apksigner verify` passed. No device/emulator is attached, so installation/runtime remain unverified |
| Open project and edit/save files | Implemented with explicit Save/Ctrl+S, dirty-tab indicator, compare-before-save protection and a close-window guard | Rust tests prove external edits are preserved on conflict and matching content saves normally. Tauri close handling asks before discarding dirty buffers or interrupting active work; actual native dialog interaction, large/read-only files and conflict recovery remain unverified |
| Git change review | Per-file diff preview added for tracked and untracked text files | Rust tests cover staged, unstaged, untracked, spaced paths, and rejecting files outside the selected workspace; native panel remains visually unverified |
| Create/switch projects | Implemented; picker and folder creation return paths, while a serialized frontend switch performs activation | Rust tests cover child-folder creation, traversal rejection and Windows junction escape. Production build passes; actual project switching during a live task still needs desktop-window verification |
| Multiple chats per project | Implemented with archive/restore and permanent local-history deletion | Each chat can be archived while preserving its transcript and OpenHands conversation ID, restored from the archive, or permanently removed with its local history. Local browser preview exposes the actions; persistence and OpenHands resume still need a native-window scenario |
| Local GGUF model flow | GPU/runtime and local Agent Server partially verified on both named models | Nemotron Cascade 8B Q6_K loaded with the signed llama.cpp Vulkan runtime on RX 7700 XT at 16K; a separate OpenHands 1.26.0 server completed a local-model text task and approved file_editor create/replace actions in a disposable workspace. It initially wrote a typo and falsely claimed exactness; an approved follow-up correction was read back from disk. Ternary Bonsai 2 27B PQ2_0 loaded with the PrismML HIP runtime at 16K and returned the exact 35-token check. Runtime-choice, prerelease asset selection, and RAM reservation have regression tests. Native Tauri model selection/auto-start and clean-install runtime download remain unverified.
| OpenHands model/task flow | Agent Server is for desktop local GGUF; remote providers use direct chat | Local Nemotron/OpenHands text and file-editor approve/reject runs were completed in isolated workspaces. Direct remote request and cancellation are unit-tested, and Kilo endpoint reachability was checked. Native Tauri local-model selection, Agent Server startup from the UI, and full task flow still need a desktop-window scenario |
| Agent action approval | Verified in isolated OpenHands runs | Kilo Free reject/approve and continuation-after-restart were previously checked; local Nemotron file_editor create and replace were approved and disk-read back. Exact results still require review: the model initially wrote the wrong content and claimed it was correct. Runtime sandbox escape and other tool classes remain unverified |
| Remote model chat | Direct provider chat on desktop and Android; no Agent Server required | Kilo Free uses the anonymous gateway; other OpenAI-compatible providers use their configured API key. Android keeps a manually entered key in memory only. Native Android use remains unverified without a device |
| Workspace access boundary | Native path checks cover symlinks and Windows junctions; backend serializes root changes with agent requests | Windows junction and agent-workspace guard tests pass; a local agent request also checks its configured workspace against the active root. OpenHands file_editor/terminal remain outside an OS sandbox |
| Change diff and agent file-editor undo | Git diff plus guarded rollback for successful OpenHands file_editor changes implemented and unit-tested | Rollback checks workspace containment and current content against the recorded after-state; terminal effects, deletions/binary changes, and remote workspace rollback are not covered. File-editor history is persisted with each local chat transcript |
| Error diagnostics and process cleanup | Partial; live OpenHands interrupt route now verified | In a disposable OpenHands 1.26.0 server with Kilo Free, an approved 45-second terminal wait was interrupted; HTTP 200, `InterruptEvent`, `paused` status, interrupted-tool error, and exited Python process were observed. Native Tauri button-to-event flow and repeated app launch/cleanup still need verification |
| Data/history persistence | Partial | Project/chat/transcript/conversation IDs and active chat selection now persist in workspace-scoped local storage; desktop reload/switch behavior and durable activity/approval journal need runtime verification |
| Public release | v1.0.16-beta.2 is published; v1.0.8 remains latest stable | The release gate verified the signed Windows setup, signature, MSI, updater manifest, and Android ARM64 APK. Public HTTP HEAD returned 200 for all five expected assets. See [the release](https://github.com/Heepean/Flux-code/releases/tag/v1.0.16-beta.2) |

## P1 — usability and reliability

- First-run setup and readiness check.
- Session/task list, resumable transcript and action journal.
- Editor dirty-state, tabs/session restore, external-change handling and diff.
- Unified provider configuration with bounded retries. OpenHands interrupt API and stop button are implemented and mock-tested; live cancellation still needs verification.
- Privacy/data destination and paid-provider cost notices at the point of use.
- Repeatable integration test suite including offline and failure cases.
- OpenHands browser-tool dependency mismatch is fixed at the resolver level (FastMCP 3.x/MCP 1.x); live browser-tool use from a Tauri-launched task remains to be verified.
- Clean-system installer and updater validation.

## P2 — expansion after P0/P1

- Heuristic model router, extensions/integrations, advanced automation, team features and optional commercial services.

## Release rule

Never mark a row verified from source inspection alone. Each release records exact commit/tag, artifact hashes, build/test results, install smoke tests, known failures, and publication links.


## Follow-up — chat archive, project list removal and search — 2026-10-12

- Chats now have separate archive and permanent-delete actions. Archive keeps local transcript and OpenHands conversation ID and offers restore; permanent deletion clears both active and archived local records. The remote OpenHands server's own history is not deleted by this local action.
- Projects can be removed from the app list without deleting their folders. Search toolbar actions target project search, focus the input, and submit the search form.
- Browser preview accessibility tree confirms the project and chat actions render. Native desktop interaction and persistent restoration are not yet exercised.

## Follow-up — desktop shell controls — 2026-10-12

- Wired both sidebar-toggle controls to collapse and reopen the project explorer/sidebar, and connected the lower settings shortcut to the model/runtime settings page. Added accessible labels and state-aware tooltips.
- `npm run build` passed. The current Vite preview confirmed both hide/show transitions and that the settings shortcut opens the model library. This is browser-preview verification; native Tauri-window behavior still needs direct validation.

## Follow-up — Windows runtime and local model verification — 2026-10-12

- Removed the prerelease filter that made current rolling llama.cpp Windows assets unavailable to clean installs. Added coverage that selects the latest compatible signed asset.
- On the AMD Radeon RX 7700 XT machine, the official SHA-256-verified Vulkan runtime loaded Nemotron Cascade 8B Q6_K at 16K context. A separately launched OpenHands 1.26.0 server using the same local OpenAI-compatible endpoint completed a text task and confirmed file_editor action approval/execution in a disposable workspace. The model made one content typo and falsely claimed correctness before a second approved replace action fixed it; inspect results for exactness.
- Prism PQ/PQ2 models select the official Windows HIP runtime on AMD adapters. This routing is covered by Rust tests; Ternary Bonsai also loaded through HIP at 16K and returned the exact test response; generation speed and a Bonsai Agent Server task remain unverified.
- GPU-offload RAM preflight is model/context-aware and unit tested. Native Tauri local-model selection/start and clean installation remain outstanding; do not consider this a release verification.
- Current checks: 30 Rust tests, Clippy (`-D warnings`), `cargo fmt --check`, `git diff --check` passed. Frontend and Tauri debug build passed after current UI edits; native window opened and responded, but model selection/start was not clicked.

## Follow-up — search and local model startup UI — 2026-10-12

- Project search now focuses its input after the tab renders, avoiding a race from a zero-delay timer.
- A selected GGUF is no longer shown as active before llama-server responds successfully. The composer button and send handler block requests while the local model is starting; if llama-server is healthy but Agent Server startup fails, the local model remains marked active for retry.
- `npm run build`, `npm run tauri -- build --debug --no-bundle`, and `git diff --check` passed after these UI changes. The rebuilt debug application opened and responded. Native interaction could not be inspected: Windows Computer Use capture timed out twice, so search clicks and model startup through the UI remain unverified.

## v1.0.16 verification update (2026-10-13)

- Remote Kilo and OpenAI-compatible chat now use a direct provider API path on desktop and Android. Remote chat does not require or start OpenHands Agent Server. Agent Server stays in the desktop local-GGUF flow; Android is remote-chat only. Android provider keys are memory-only and must be entered again after restarting the app.
- uv ZIP lookup now scans archive entries by index; regression coverage includes flat and nested executable paths. Direct remote HTTP request and cancellation have isolated tests.
- `npm run build`, `cargo test` (33 tests), `cargo clippy --all-targets -- -D warnings`, and `git diff --check` pass. Android target cross-check passes with NDK 27.2.12479018. Tauri CLI compiled the ARM64 library but could not create its symlink because Windows Developer Mode is off; copying that library and running Gradle produced a 192,105,459-byte ARM64 debug APK, and `apksigner verify` passed (package `dev.fluxcode.fluxhands`, versionCode 1001016, minSdk 35). No Android device is attached, so installation/runtime are not yet verified.
- All platform app icons were regenerated from the supplied Flux Hands source emblem. Windows setup and MSI built locally; signed Windows setup, updater signature, MSI, updater manifest, and Android ARM64 debug APK were uploaded by the successful v1.0.16-beta.2 release workflow. GitHub reports HTTP 200 for all five expected assets. The APK signature was verified in CI and locally. Android installation and Windows clean-install/upgrade are still unverified on physical devices.
