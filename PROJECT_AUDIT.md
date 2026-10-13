# Flux Code / Flux Hands project audit

> **Current release status (2026-10-12):** The historical snapshots below record what was known at each audit point. The latest follow-up at the end of this file supersedes earlier statements that v1.0.15 was unpublished: `v1.0.15-beta.1` is public with Windows assets and an Android ARM64 APK. The APK has not been installed on a device.

Audit snapshot: 2026-10-12, local checkout `D:\Flux code\repo-check` at `f0fb1ad` with a clean worktree; `v1.0.14-beta.2` is running Actions workflow 38220704939 for earlier commit `0634555`. Flux Code and Flux Hands refer to this one project. This is a source/build and focused runtime audit, not a claim that every workflow was exercised on a clean user machine.

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
| Create and switch projects | Implemented in UI and native commands | `cargo test` covers safe child-folder creation and traversal rejection; local browser preview showed the open-existing/create-new dialog. Native folder-picker and full Tauri project-switch flow are not yet exercised |
| Multiple chats per project | Implemented with transcript and OpenHands conversation-ID persistence | Local browser preview showed new chat creation; switching, reload restoration and independent server continuation have not yet been exercised in the desktop UI |
| Save editor content | Explicit Save/Ctrl+S, per-tab dirty state | `npm run build` passed; native file editing/save, failed-save recovery, external changes and reload behavior remain unverified |
| Workspace text search | Implemented in Rust; skips selected directories and large/binary files | Rust unit tests pass (15 total at `f0fb1ad`) |
| Git branch/status and changed file opening | Implemented (read-only status) | Rust temp-repository test passes |
| Local GGUF launch/switch | Implemented with CPU/Vulkan runtime bootstrap, model-format-aware PrismML selection, RAM check and one child process | On RX 7700 XT, Nemotron Cascade 8B Q6_K passed a local completion on CPU (~6 tokens/s) and Vulkan GPU (~57 tokens/s). Ternary Bonsai 2 27B PQ2_0 required PrismML; on Vulkan it loaded and passed a correct 2+2 answer (~40 tokens/s). PrismML CPU also loaded and completed, but consumed nearly all available RAM (free RAM fell to ~0.34 GB) and was slow (~4 tokens/s). These were direct llama-server runs, not yet through the native app's new runtime bootstrap |
| Hugging Face model search/download | Implemented for desktop | Source reviewed; no network/download scenario run here |
| OpenHands local/remote task execution | Implemented using API/event polling; interrupt support is included in `f0fb1ad` | Earlier dev-app E2E covered Kilo Free events and approval. Interrupt endpoint path, session header, and remote/local base selection pass an HTTP mock/unit test; cancellation against a running live conversation is not yet verified |
| Kilo free catalog | Implemented against live Kilo endpoint | Anonymous Kilo Auto Free request through OpenHands 1.26.0 returned the expected marker with zero reported model cost; tool-capable action flow and provider limits remain in scope |
| Android remote agent flow | Implemented | 1.0.13 ARM64 debug APK built; signature and package metadata checked; no phone/emulator attached, so not installed/launched |
| Auto-update | Configured for GitHub Releases, signed updater manifest expected | Not exercised against a published release; local Windows installers lack updater signatures |
| Git per-file diff preview | Implemented in `f0fb1ad` | Rust tests cover tracked staged/unstaged changes, untracked text files, filenames with spaces, and rejecting a sibling path outside the workspace; native panel not visually verified |
| OpenHands file_editor rollback | Implemented in `f0fb1ad` | Successful observations carry before/after state; rollback is limited to the active local workspace and only applies when current bytes still equal the recorded after-state. Unit tests cover restoration, deleting an agent-created file, preserving subsequent user edits, and rejecting outside paths. Currently persisted only with the chat transcript in app local storage; does not undo terminal side effects or remote files |
| Terminal panel and terminal-command rollback | Not implemented | A terminal command may have arbitrary external side effects; no safe automatic undo is claimed |
| Android user experience | Remote-only flow implemented | APK is ARM64 debug-signed, not Play Store/release signed |

## Critical findings and risks

### P0 — Agent action confirmation needs real runtime proof

The source originally sent `NeverConfirm`. New conversations use `AlwaysConfirm`; reused conversations call the Agent Server confirmation-policy endpoint, poll pending ActionEvents and present tool/action details with explicit approve/reject controls. A dev-app E2E test against OpenHands 1.26.0 and Kilo Free verified both outcomes in a disposable workspace: the file did not exist before either decision; rejection left it absent and reported that execution stopped; approval created only the proposed file with the requested bytes. The conversation also resumed after the Agent Server process was restarted and returned `CONVERSATION_CONTINUED_OK`, exercising the camelCase continuation and confirmation command arguments. That preload error came from FastMCP 4 resolving MCP 2 against browser-use's MCP 1 decorator API. The runtime pins FastMCP 3/MCP 1; a fresh OpenHands 1.26.0 server now starts browser-tool preload successfully. Remaining: prove the runtime cannot escape its workspace, test confirmation across other tool classes, and run file-editor rollback against a live Tauri/OpenHands task.

### P0 — Workspace boundary only covers Flux Hands file commands

Desktop native state records the active root from the OS folder picker and persists it; workspace file/list/search/Git/model-download commands no longer accept a caller-supplied root. Targets are canonicalized and rejected outside that active root, and editor saves only affect existing files. Unit coverage verifies that a sibling path and a symlink escape are rejected where symlinks are supported. Windows junction escape and root-switch races still need coverage. This does not sandbox OpenHands' independent runtime or terminal tool; test and document that boundary separately.

### P0 — Tauri command argument casing broke agent conversation continuation

`send_agent_message` and `respond_to_agent_confirmation` declared Rust `conversation_id`, but the UI sends Tauri arguments as `conversationId`. That meant the first message path omitted the optional field harmlessly, while continuing an existing conversation and responding to approval could fail argument decoding. The commands now accept the frontend camelCase spelling. A real Agent Server message invocation using `conversationId: null` succeeded after this fix; continuation and approval endpoints still need a dedicated live test.

### P0 — Local model command mismatch fixed; broad provider verification remains

The UI sent camelCase `modelPath` and `gpuLayers` to `start_local_model`; Rust expected snake_case in the nested struct, so UI-triggered GGUF startup failed before spawning the server. `StartModel` now deserializes camelCase and a regression test covers this. A current local OpenHands 1.26.0 + Nemotron Vulkan run at 16K context answered and completed file_editor create/str_replace actions in a disposable workspace after approvals. The first action had a content typo and the model incorrectly claimed it was exact; a follow-up generated the correct replacement, which was independently read back from disk. Native Tauri model-selection/auto-start and clean installer flows remain unverified.

### P1 — UI state persistence needs runtime validation

The UI now stores project lists and per-project chat transcripts plus OpenHands conversation IDs in `localStorage`, and migrates the previous single conversation ID. Reload restoration, large transcripts, browser storage failure and independent continuation per chat have not yet been exercised in the desktop UI. Pending approvals and a durable task/event journal are still missing; successful file-editor before/after records are now saved inside each chat transcript, but this persistence has only build-level verification so far.

### P1 — Editor is intentionally minimal

Monaco is present with a Save button, Ctrl+S and visible dirty indicators. There is no tab/session restore, save conflict detection for external edits, or navigation/search integration in the editor. Git diff is available in the Git panel; guarded file-editor rollback is available for local agent actions. Native edit/save flow has not yet been exercised in a desktop window.

### P1 — Task lifecycle and cancellation remain incomplete

The current working tree adds a stop button and calls OpenHands 1.26.0 `POST /api/conversations/{id}/interrupt` for local and remote servers. HTTP mock coverage verifies the route and session header, and terminal `paused` state is surfaced as user cancellation. A running-agent cancellation scenario has not yet been exercised end to end. The runtime isolation gap also remains: native file commands enforce the selected root, but OpenHands LocalWorkspace and terminal run directly as the desktop user. Task list, structured durable activity/approval journal, and restart recovery remain missing.

### P1 — Release maturity differs by platform

The GitHub workflow creates signed Windows updater assets and an ARM64 debug APK. Public stable v1.0.8 currently reports six release assets; the public page does not expose their names in this audit. Candidate v1.0.14-beta.2 (workflow 38220704939) still reports In progress and no workflow artifacts in the public Actions page; stable v1.0.8 remains the newest listed release. The candidate does not contain later source commits including `bfafbac`, `90eaadd`, or `f0fb1ad`. Its Android job runs after the Windows release job. The previous Android job failed during SDK setup. Local Windows NSIS/MSI builds and APK are unsigned/debug-signed respectively; neither has been installed and smoke-tested on a clean Windows PC or Android device.

### P1 — Product says “no account/no limits” only within provider constraints

Local inference needs hardware/runtime; Kilo's anonymous free tier has upstream rate limits; remote providers can charge and retain prompts. Existing README discloses these constraints. Maintain this accuracy in UI and release copy.

## Build and repository state

- `npm run build`: passed on 2026-10-12 (TypeScript compile and Vite production build).
- `cargo test --manifest-path src-tauri/Cargo.toml`: passed 15 tests in the current working tree on 2026-10-12, including Windows child-process-tree termination, project-folder traversal rejection, mocked OpenHands interrupt routing, and Git diff stage/scope cases.
- Latest `npm run build`, `cargo check --manifest-path src-tauri/Cargo.toml`, `cargo fmt --check`, and `git diff --check` passed on 2026-10-12.
- `npm run build`: passed after the local model command fix.
- Windows NSIS (3,815,192 bytes) and MSI (5,677,056 bytes) v1.0.13 installers built locally. Tauri updater signing failed because the private signing key is only available in GitHub Actions; no local Authenticode signature was established.
- Android ARM64 debug APK built locally at `src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk` (SHA256 `E2AC4B5C84AA7023C801B8235D1D7D46E1B480A289361137653AF9C9265E2A9B`); apksigner v2 verification and package/version metadata passed. No Android device/emulator was attached.
- llama.cpp CPU b11429 archive SHA-256 matched upstream GitHub attestation; temporary Llama 3.2 1B Q4_K_M model SHA-256 matched Hugging Face metadata. Dev app returned `FLUX_TAURI_OK` through its native local-model flow.
- Nemotron-Cascade-8B-Q6_K was rejected before spawn: about 5.0 GB RAM free vs. about 8.3 GB required.
- Local Agent Server E2E: app launched pinned OpenHands 1.26.0 against a disposable workspace, Kilo Free returned `FLUX_OPENHANDS_OK`, events carried the conversation ID and terminal status, and `stop_agent_server` completed. This predates the interrupt UI; live interrupt remains unverified. The original workspace configuration was restored after the test.
- Kilo Auto Free probe: a direct pinned OpenHands 1.26.0 server in a disposable workspace accepted `openai/kilo-auto/free`, returned `KILO_AUTO_PROBE_OK`, wrote `execution_status=finished` to its event store, and was stopped with the listening port closed. This validates the model ID/gateway path, not the full Tauri UI send flow.
- Process-tree regression: a Windows test launched a wrapper and child process, ran `terminate_process_tree`, verified the child PID exited, and passed; the production Agent Server path uses this helper.
- New project/chat UI was checked in a local browser preview; Tauri native folder dialogs and desktop persistence remain unverified.
- This OpenHands startup produced a non-fatal browser tool preload error (`Server.list_tools` missing); server reached ready state and the no-tools response succeeded. Browser automation tools may be unavailable until the upstream dependency mismatch is resolved.
- Public Actions page still reports workflow 38220704939 for v1.0.14-beta.2 as In progress with no workflow artifacts; GitHub Releases still lists v1.0.8 as newest with six assets. The beta has not produced a verified installer or APK.
- Clean-machine installed-app smoke test: not performed.

## Immediate implementation order

1. Exercise approval and rejection with a live OpenHands 1.26 test server; verify that no action runs before approval and rejected runs visibly stop.
2. Publish a CI run with separated Android license and SDK installation steps; finish APK build and verify release assets.
3. Add Windows junction/root-switch race coverage to the selected-workspace boundary; establish and test the OpenHands runtime's actual containment.
4. Run the new stop flow against a live running OpenHands 1.26.0 task; then add durable event/transcript state and test cleanup on app exit and failure.
5. Implement agent change preview (diff) and rollback based on recorded file state.
6. Run repeatable integration scenarios for local/remote OpenHands, provider switching, and Android remote connection.
7. Complete editor/session persistence and external file change handling.
8. Install-test official installers on clean Windows and Android targets before any stable readiness claim.

## Scope limits

This audit reviewed source, CI/release status and artifacts, ran a real small-GGUF inference request through Tauri native commands, and exercised OpenHands 1.26.0 through the dev app in earlier approval scenarios and directly for the Kilo Auto Free model-path check. It did not install the official desktop release on a clean Windows PC, connect an Android device, exercise the new project/chat UI in a desktop window, or conduct a security penetration test. Statuses must be updated when those checks are carried out.

## Follow-up verification — 2026-10-12

- Current source is local commit `e3806cf` (`Publish Android APK independently in v1.0.15`), based on `e948379`. It is one commit ahead of `origin/main`; the annotated tag `v1.0.15-beta.1` exists locally but neither commit nor tag has been pushed because GitHub credentials are unavailable in this environment.
- GitHub's public Releases page was rechecked: v1.0.8 remains the newest listed release. No v1.0.15-beta.1 release or APK is public yet.
- The release workflow now creates a release entry first and makes Android and Windows build jobs independent. This workflow change is in the unpushed commit; no hosted Actions run has validated it.
- `npm run build`, `cargo fmt --check --manifest-path src-tauri/Cargo.toml`, and `cargo test --manifest-path src-tauri/Cargo.toml` passed on this source; 15 Rust tests passed.
- The optimized Android ARM64 release APK was built from the current v1.0.15 source. Tauri's Windows CLI compiled the Android Rust library, then stopped because Windows denied symlink creation; copying that compiled `.so` into generated `jniLibs` and running Gradle `:app:assembleArm64Release` completed successfully. The unsigned APK was signed with the local Android debug keystore for testing. The delivered artifact is 17,146,731 bytes; package `dev.fluxcode.fluxhands`, version `1.0.15`, versionCode `1001015`, minSdk 35; `apksigner verify` passed. SHA-256: `5D787823827E2069B81D0B8940EF9563EB9AAB73A34BC2382490C30A1FE04549`.
- No Android device or emulator is attached; installation and runtime behavior are unverified. The local debug signing key is not the production updater or a durable release signing key. Windows installers for v1.0.15 have not been built or published.
- Android local-build output and signed APK are in the ignored generated Android tree and untracked `artifacts/` directory respectively; the APK is not committed to source.

## Follow-up verification — Windows workspace boundaries — 2026-10-12

- Added a Windows-only unit test that creates an NTFS junction from the selected project into a sibling directory, checks that resolving a file through the junction is rejected, and verifies the outside file is unchanged. The test passed on this Windows machine.
- Project switching holds a frontend lock until the new root, file listing and model scan finish loading; sending a task, switching chats or starting another switch is rejected while that transition is active. Folder selection and project-folder creation return a path without changing native active-workspace state; the common switch routine performs activation.
- The current working changes add a native RAII agent-workspace guard: a running `send_agent_message` holds it across awaits, root changes are rejected until completion/cancellation/error, and overlapping agent messages are rejected. Local-server sends also reject a configured workspace that no longer equals the active root. The unit test verifies lock acquisition, root-switch rejection and release.
- `npm run build`, `cargo fmt --check --manifest-path src-tauri/Cargo.toml`, `cargo test --manifest-path src-tauri/Cargo.toml`, and `git diff --check` passed; the Rust suite now has 17 passing tests.
- The UI/native live switch-during-task scenario has not yet been exercised. This protects workspace state from direct Tauri path-switch calls while a request is active, but OpenHands' local runtime and terminal still run with the user's Windows permissions and have no OS-level sandbox.

## Follow-up verification — live OpenHands cancellation — 2026-10-12

- Started the pinned OpenHands Agent Server 1.26.0 with Kilo Free in a disposable workspace under `%TEMP%`. The model proposed exactly one terminal action: a 45-second Python sleep followed by a marker print. The pending action was inspected and approved.
- Sent `POST /api/conversations/{id}/interrupt` with the session API key while the action was running. OpenHands returned HTTP 200, emitted `InterruptEvent`, recorded `execution_status=paused`, and emitted `AgentErrorEvent` with `Tool call interrupted before completion. The conversation was paused.` No Python process for the sleep remained, and the marker was not printed.
- The Agent Server process was stopped and port 51265 was confirmed closed. This verifies the pinned server's live route and process cancellation, not the Tauri button-to-command/event path or Android remote cancellation.
- The temporary test directory remains at `C:\Users\Admin\AppData\Local\Temp\flux-hands-live-cancel-368653b52eaf4315afcbce2127c76044`; recursive removal was blocked by the execution policy. It contains only the isolated test workspace and OpenHands persistence from this probe.

## Follow-up verification — Android release asset — 2026-10-12

- Queried the public GitHub Releases API: `v1.0.8` is still latest and its four uploaded assets are the Windows installer, updater signature, MSI and `latest.json`; there is no APK, despite the release description saying one is attached.
- GitHub Actions run `38220386102` for tag `v1.0.8` completed the Windows job successfully, but Android failed at `android-actions/setup-android@v3`. Its SDK, build, APK preparation and upload steps were skipped. Earlier run `38168245657` failed at the same Android setup step. This explains how the release became public without an APK.
- Updated the local release workflow: create the release as a draft; upload Windows and Android assets while it stays a draft; publish only after a final job verifies the expected four Windows/updater files and non-empty Android APK. Older asset cleanup now runs only after that publication gate. The Android setup action is v4 in this local workflow.
- Parsed the workflow with PyYAML and asserted that `publish_release` requires both platform jobs and all three release-action steps keep the release in draft mode. The hosted workflow has not run with these changes because they are still local and GitHub credentials are unavailable.
- Revalidated `npm run build`, `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, and `cargo test --manifest-path src-tauri/Cargo.toml`: build and formatting passed; all 17 Rust tests passed. The APK remains locally signed with a debug key and device installation is still unverified.

## Follow-up verification — Android APK rebuilt from current source — 2026-10-12

- Rebuilt the Android ARM64 Rust library from commit `d5e2465` in both debug and optimized release profiles. Tauri then stopped at its Windows symlink step; copied the just-built release `.so` into the generated Android project's `jniLibs` and assembled `:app:assembleArm64Release` with that Rust build task skipped. Gradle completed successfully.
- Signed the optimized APK with this machine's Android debug keystore. `apksigner verify`, `zipalign -c`, and `aapt dump badging` passed. The APK's native library SHA-256 matches Gradle's stripped release-library output.
- Current artifact: `artifacts/Flux.Hands_1.0.15-beta.1_android-arm64.apk`, 17,150,827 bytes; package `dev.fluxcode.fluxhands`, version `1.0.15`, versionCode `1001015`, minSdk `35`, targetSdk `37`; SHA-256 `134956B97587DAF3DDCCB4E5B339E9E1A1DB3849BF88E68763E40F0055E3A370`.
- `adb devices -l` reports no connected device or emulator. Installation, first launch, remote Agent Server setup and a real Android task are therefore still unverified. This locally debug-signed APK is for testing and is not a production-signed public release.

## Follow-up verification — Android CI SDK package ID — 2026-10-12

- Pushed commit `018e34a` and tag `v1.0.15-beta.1`; hosted run `38227227774` executed the new draft-gated workflow. `prepare_release` passed and created a draft; Android setup v4, license acceptance and platform-tools installation passed. Android then failed at the requested SDK platform install before NDK/build/upload. The draft gate kept it unpublished.
- Queried the installed Android SDK Manager package catalog: there is no `platforms;android-37` package ID. The available matching platform IDs are `platforms;android-37.0`, `.1`, and `.2`; the project uses compileSdk/targetSdk 37 and this workstation builds against 37.0. Updated the workflow to install `platforms;android-37.0`.
- Windows job in run `38227227774` completed the signed installer build and asset upload, then failed its own asset-name assertion. The check expected `Flux Hands_1.0.15_x64_en-US.msi`; the public v1.0.8 asset demonstrates the actual `Flux.Hands_1.0.8_x64_en-US.msi` naming convention. Corrected the expected MSI name to `Flux.Hands_...` in both the Windows check and final publish gate.
- Updated workflow fixes for the SDK package and MSI name are local but not yet pushed, so neither has been validated by another hosted Actions run. The draft remains unpublished pending that run.

## Follow-up verification — release asset gate — 2026-10-12

- Forced the `v1.0.15-beta.1` tag to the corrected SDK/MSI checks and started run `38228369458`. Android installed API 37.0, built the ARM64 APK, verified its signature and uploaded it. Windows built signed installers and uploaded them. The release remained a draft as intended.
- Both jobs then failed only in their duplicate per-job GitHub asset-list checks; the final publish job was skipped. Public check-run annotations show generic exit code only. The hosted build/upload steps themselves succeeded.
- Reworked the local workflow to use `softprops/action-gh-release@v3`, explicitly overwrite assets on reruns, remove the two parallel list checks, and perform a single post-build check with retries. The gate requires setup, signature, updater manifest, APK, exactly one version-matching MSI (accepting space or dot product-name separators), and positive asset sizes before publishing.
- The updated workflow is locally YAML-validated but has not yet run in GitHub Actions. v1.0.15-beta.1 remains an unpublished draft; the public latest release remains v1.0.8.

## Follow-up verification — latest release attempt and project/chat UI — 2026-10-12

- Commit `3e10f53` was present on both `origin/main` and tag `v1.0.15-beta.1` when hosted workflow `38229581194` ran. `prepare_release`, Windows build/upload, and Android SDK/build/signature/upload succeeded. The final gate failed because `GET /releases/tags/v1.0.15-beta.1` returned HTTP 404 for the draft release; the exact error was recovered from authenticated Actions logs. Cleanup was skipped, the public latest release remains v1.0.8, and the v1.0.15 tag page has no APK asset.
- Corrected the local gate to use the release ID returned by the draft-creation action and JSON asset metadata returned by both platform upload actions, then publish by release ID and verify GitHub's PATCH response. YAML parsing, PowerShell AST parsing, synthetic five-asset gate execution, and `git diff --check` pass. This correction is not yet in a hosted Actions run.

## Follow-up verification — public Android and Windows prerelease — 2026-10-12

- Run `38231141807` proved the corrected asset gate: the release draft action returned ID `410404593`; Windows produced four non-empty assets and Android uploaded a signed ARM64 debug APK; the gate published `v1.0.15-beta.1` and verified GitHub's response contained all five required assets with positive sizes.
- The hosted run ended failed only after publication: `Remove installers from older releases` could not find `scripts/cleanup-old-release-assets.ps1` because the `publish_release` job did not check out the repository. The public release is already available; added checkout to the workflow locally so the cleanup step can run in future releases. Older-release asset cleanup has not yet been performed.
- Public page lists the APK `Flux.Hands_v1.0.15-beta.1_android-arm64-debug.apk` at 179,624,315 bytes, SHA-256 `aa082df4ea197a24a47d02ba673b66c1f3e8a1880e5844e949c63e503235529c`; its page and release download URL are public, and HEAD returned HTTP 200 with `application/vnd.android.package-archive`. The same page lists setup EXE, signature, MSI, and updater manifest with digests. The release is a prerelease; stable remains v1.0.8.
- The uploaded APK is 171 MB and debug-signed for testing. CI signature verification passed, but no Android device is connected; installation and runtime behavior remain unverified. The local optimized debug-signed candidate remains a separate 17 MB file and is not the uploaded CI artifact.
- Production frontend build passed (`npm run build`) and the complete native Rust suite passed (17 tests). This includes safe project-folder creation/path traversal rejection and workspace boundary tests.
- In a local browser preview, created a new chat and confirmed it appeared in the chat list after a page reload. The project dialog showed both “Открыть существующую папку” and the new-folder flow. This verifies browser UI and local chat persistence only; native folder-picker, directory activation, and desktop transcript switching remain unverified.
- The optimized Android APK remains a local candidate at `artifacts/Flux.Hands_1.0.15-beta.1_android-arm64.apk`, separate from the larger debug APK uploaded to the public prerelease above.

## Follow-up — per-project active chat restoration — 2026-10-12

- Persisted the selected chat ID under a workspace-specific local-storage key and restore that conversation on project activation/reload, falling back to that workspace's most recent chat if the saved ID no longer exists.
- `npm run build` and `git diff --check` pass. This validates compilation and patch formatting only; native desktop restart/switch behavior remains unverified. This checkout does not define the `lint:ui` npm script.

## Follow-up — local model switch memory preflight — 2026-10-12

- Fixed `start_local_model` to terminate and wait for the previous llama.cpp process before checking available RAM for the selected model. The model child lock remains held across shutdown and the preflight, so two model starts cannot overlap this transition.
- Added a Windows regression test that starts a disposable process and verifies the preflight callback only runs after that process has been stopped. `cargo test --manifest-path src-tauri/Cargo.toml` passed all 18 tests; strict `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `npm run build`, and `git diff --check` passed.
- 2026-10-12 follow-up: detected AMD Radeon RX 7700 XT (12,272 MiB Vulkan memory) and 15.6 GB system RAM. Direct local inference succeeded for both user GGUFs on CPU and GPU. The 27B PQ2_0 model needs PrismML; upstream llama.cpp rejects GGUF tensor type 142. Added CPU/Vulkan backend selection and verified-hash runtime bootstrap, with PrismML selected for PQ2_0/PTQ1_0. Production build, 18 Rust tests, strict Clippy and formatting passed. Native Tauri invocation of the new installer path and clean-install runtime download still need verification. Keep context at a practical value (default 4096); context 512 was enough for arithmetic but truncated a longer explanation.


## Follow-up — editor dirty-state protection — 2026-10-12

- Switching projects now warns before discarding unsaved edits and clears old workspace buffers only after the switch is accepted. Closing a dirty editor tab also asks before discarding its in-memory draft; rejecting the prompt keeps the tab open.
- `npm run build`, all 18 Rust tests, strict Clippy, Rust formatting and `git diff --check` passed. Native confirmation-dialog interactions remain unverified because the available computer-use surface does not expose the Tauri window.
pm run build, all 18 Rust tests, strict Clippy, Rust formatting and git diff --check passed. Native confirmation-dialog interactions remain unverified because the available computer-use surface does not expose the Tauri window.
- Fixed the copied command prefix and ran `npm run build`, `cargo fmt --check`, all 18 Rust tests, strict Clippy and `git diff --check` successfully.

## Follow-up — local Agent Server startup and context — 2026-10-12

- Desktop local-model flow now waits for llama-server readiness and then starts/reuses the OpenHands Agent Server with the active workspace and local model endpoint. If Agent Server startup fails, the UI keeps the model running and reports the partial-ready state. Android remains remote-server-only.
- Raised the local-model default/minimum context to 16,384 tokens. An 8,192-token Nemotron Cascade 8B Q6_K run truncated the OpenHands tool prompt; with 16,384, OpenHands 1.26.0 generated a real `TerminalAction` and paused at confirmation. The attempted follow-up response targeted the previous rejected action, so an approved tool execution is not claimed.
- Direct process integration confirmed Vulkan llama-server plus OpenHands Agent Server health and conversation creation on this Windows/RX 7700 XT machine. This was a manually launched runtime probe, not a Tauri UI/native auto-start test. Release assets and Android remote-device behavior were not tested in this pass.
- `npm run build`, all 18 Rust tests, strict Clippy, Rust formatting and `git diff --check` pass.

## Follow-up — refreshed Flux Hands application icons — 2026-10-12

- Replaced the Tauri PNG/ICO icon set with the image supplied by the user. Kept the original image and an Android icon manifest/background so the launcher artwork can be regenerated consistently. Release workflow now regenerates platform icons after Android project initialization.
- `npm run build` and `npm run tauri -- build --debug --no-bundle` passed; the Windows app binary was produced with the refreshed icon assets. Android launcher resources were regenerated, but local APK packaging stopped when Windows denied creation of the Android `.so` symbolic link (`Developer Mode`/symlink privilege is disabled). No new APK or public release is claimed.

## Follow-up — local Nemotron Agent Server approval cycle — 2026-10-12

- Started the user's Nemotron Cascade 8B Q6_K GGUF through Vulkan llama-server (`n_ctx=16384`) and OpenHands Agent Server 1.26.0 against a new disposable workspace. Server health returned `ok`; the conversation reached `waiting_for_confirmation` with the proposed action `Write-Output 'FLUX_APPROVED_EXECUTED'`.
- Posted approval only after verifying that conversation was waiting. OpenHands returned HTTP 200, executed the proposed PowerShell command, emitted a terminal `ObservationEvent` with `FLUX_APPROVED_EXECUTED`, and finished the conversation. This closes the earlier incorrect approval-target attempt for the direct local runtime path.
- Added a Rust unit test covering the app's approval route, API-key header, and `accept=true` payload. `cargo test` passed 19 tests; `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `npm run build`, and `git diff --check` pass. Temporary Agent Server and llama.cpp processes were stopped.
- The live cycle used manually launched processes and direct HTTP, not the native Tauri window. Automatic local model→Agent Server startup and clicking approval in the desktop UI remain unverified; do not treat this as full app E2E.

## Follow-up — OpenHands runtime bootstrap and editor conflict protection — 2026-10-12

- The pinned uv bootstrap validates the official GitHub SHA-256 (or Astral sidecar fallback), limits archive and executable sizes, stages extraction, accepts flat and legacy nested uv archive layouts, and restores both prior executables if activation fails. Tests cover API/sidecar checksums, archive path layouts, and rollback when the second binary is missing.
- Downloaded uv 0.13.0's Windows x64 archive and independently verified its 15,722,003-byte payload digest against GitHub API metadata and the published sidecar. The actual release stores `uv.exe` and `uvx.exe` at the archive root; the loader now supports that layout. A unit test confirms both flat and nested names resolve.
- The actual installed OpenHands 1.26.0 package cache confirms new conversations use `LocalWorkspace`; its workspace path is not an OS sandbox and agent terminal commands run under the desktop user's account. Native Flux Hands file commands enforce the selected root, but OpenHands commands do not. The device has no Docker CLI, so Docker isolation has not been implemented or verified.
- Native editor saves now send the version originally opened. Rust rejects save if the file changed on disk and preserves the external bytes; the UI retains the unsaved draft and explains close/reopen recovery. Two tests cover conflict preservation and the ordinary save path. The actual Tauri save/recovery interaction is not yet exercised.
- `npm run build`, `cargo test` (25 tests), strict Clippy, Rust formatting, and `git diff --check` passed. Existing local modifications remain uncommitted.
- Rebuilt the debug Tauri desktop executable from this working tree (`npm run tauri -- build --debug --no-bundle`) after the editor change and added agent-permission wording beside the task composer. Computer-use reports no native apps/windows, so the generated desktop binary was not launched or UI-tested; the audit evidence remains code/build/test level for these UI changes.
- Public GitHub Releases API currently shows v1.0.15-beta.1 with five non-empty assets and v1.0.8 as latest stable with four; all intermediate releases have no assets. Cleanup script dry-run against the live public API kept both active releases and found zero old installer assets to delete. `gh` CLI is not installed, so direct authenticated cleanup or upload cannot be done from this environment.
- Cleanup script now supports PowerShell `-WhatIf`, performs read-only preview without a GitHub token, and refuses to delete when the current release remains a draft. Live `-WhatIf` confirmed the current public release set requires no removals.
- Tauri now intercepts desktop-window close while an editor buffer is dirty, an agent task runs, or a model is starting; users can cancel closing or explicitly confirm discarding/interrupting. This is source-level only until a native window is available.

## Follow-up — desktop shell controls — 2026-10-12

- Replaced inert sidebar and settings controls with working actions. Both sidebar buttons hide/reopen the explorer panel; the bottom settings control opens model/runtime configuration.
- `npm run build` passed, and the Vite preview verified collapse, restore, and settings navigation. This does not replace a native Tauri-window smoke test.


## Follow-up — browser-tool dependency compatibility — 2026-10-12

- Pinned the OpenHands 1.26.0 Agent Server's FastMCP/MCP dependencies to compatible major versions. The resolved package set now initializes the installed browser-use MCP server; this fixes the known preload exception at the package/API level.
- Verified with 26 Rust unit tests, Clippy with warnings denied, cargo fmt --check, and git diff --check. Native Tauri startup and browser interaction in an actual agent task remain unverified.

## Follow-up — rolling llama.cpp install, AMD GPU and local OpenHands — 2026-10-12

- The llama.cpp GitHub API currently returns rolling Windows assets as prereleases; selecting only non-prerelease builds left a fresh install with no runtime candidate. Runtime selection now accepts the newest compatible asset from the release list and still requires GitHub's SHA-256 before extraction. A regression test covers the rolling prerelease asset. Current Windows Vulkan asset `llama-b11565-bin-win-vulkan-x64.zip` (32,084,447 bytes) matched API digest `d21649a9cc9dc063d88848ffa9ae3986222c9e953f2f950e993e00e49cedfe21`.
- Runtime selection detects an AMD Radeon adapter and routes Prism PQ1/PQ2 models to the Windows HIP asset. Ordinary GGUF models continue to use Vulkan. This machine reports AMD Radeon RX 7700 XT; its downloaded Vulkan build loaded the real Nemotron Cascade 8B Q6_K at 16,384 context, served `/v1/models`, and generated at roughly 33–49 tokens/second under the test prompts.
- A separate OpenHands 1.26.0 server started with the pinned FastMCP 3/MCP 1 environment, initialized browser-tool preload, and used the local Vulkan endpoint. A simple task finished. In a disposable temporary workspace, OpenHands proposed `file_editor.create`, waited for confirmation, and created one file; its first content contained a typo and its completion message falsely called it exact. A follow-up proposed `file_editor.str_replace`; after approval, disk readback confirmed exact `LOCAL_OPENHANDS_OK`. This demonstrates that the integration and approval path work, while local model output still needs review and independent verification.
- Local RAM check now accounts for GPU-offloaded weights and the selected context rather than always requiring the full GGUF file size in system RAM. A unit test covers CPU versus full-GPU reserve and larger-context behavior. The actual preflight command through Tauri remains unverified; during the live 16K Vulkan+OpenHands run, available system RAM was about 2.3 GB, so this remains a low-memory device and CPU-only use of these large models needs substantially more headroom.
- Verified 30 Rust tests, Clippy with warnings denied, `cargo fmt --check`, and `git diff --check`. The Tauri app window launched and responded, but native selection/click-to-start was not exercised. The GitHub runtime download/install command is covered by source tests and the manually verified asset, not yet by a clean-install smoke test.

## Follow-up — v1.0.16-beta.2 release and product-page correction — 2026-10-13

- The Android ARM64 release job initially failed because `is_amd_radeon_adapter` was incorrectly gated to Windows while shared runtime-selection code also compiled on Android. Removed the platform gate; `cargo check --target aarch64-linux-android` now passes with NDK 27.2.12479018.
- Local Tauri Android build compiled the ARM64 Rust library but could not create its symlink because Windows Developer Mode is disabled. Copying the compiled library into generated `jniLibs` and running Gradle `:app:assembleArm64Debug -x :app:rustBuildArm64Debug` produced an APK; local `apksigner verify` passed. The published APK was separately signature-verified by CI. No physical Android install was performed.
- Published release `v1.0.16-beta.2` includes Windows setup EXE, updater signature, MSI, `latest.json`, and Android ARM64 APK. Each expected public URL returned HTTP 200. GitHub's release workflow published only after its asset gate passed.
- Current checked source passes `npm run build`, all 33 Rust tests, strict Clippy, `cargo fmt --check`, and Android ARM64 cross-check. Windows setup/MSI built locally; clean install, upgrade, and desktop-native task interaction remain unverified.
- Updated README to document the actual platform split: Windows local GGUF tasks use Agent Server; direct remote chat requires no Agent Server; Android supports remote chat only and does not access local project files. The screenshot remains the supplied Flux Hands workspace image.