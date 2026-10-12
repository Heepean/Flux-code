# Testing strategy

## Evidence levels

Record these independently: source inspection, unit tests, production build, runtime smoke test, clean-install test, signed release verification, and real provider/device integration. A successful earlier level does not imply a later one.

## Required P0 scenarios

### Workspace and editor

- Open a normal project; expand nested folders; read, edit and save a text file.
- Reject `..`, another drive, a symlink/junction escape, invalid encoding, locked files, and a stale workspace root.
- Verify failed saves are surfaced and never reported as saved.
- Test a large project, binary files, and files modified externally while open.

### Local model

- Start a small known GGUF, wait for `llama-server` health, send a request through OpenHands, switch models, and confirm only one model process remains.
- Test missing/corrupt GGUF, unavailable runtime, insufficient RAM, wrong endpoint, timeout, cancellation, and app exit cleanup.

### OpenHands and providers

- Run with OpenHands 1.26 and a deterministic mock OpenAI-compatible provider.
- Verify conversation creation, streamed events, tool proposal, no execution before user approval, approved execution, rejected execution, and errors/timeouts.
- Switch local/remote/Kilo providers and verify the same conversation history remains available.
- Verify unsupported confirmation endpoints fail closed.

### Android companion

- Install the ARM64 APK on a physical Android 15+ device.
- Connect to an HTTPS server, verify credential persistence and reconnect, send a task, approve/reject a pending action, and test offline/server failure states.
- Confirm no desktop-only local GGUF controls are accessible.

### Distribution

- Install NSIS and MSI on a clean supported Windows VM, launch, open a project, and uninstall without deleting user workspace files.
- Verify updater signature, update from the previous released version, preserve settings, and recover from interrupted download/install.
- Install the production-signed APK and verify upgrade from a previous release.

## Current automated coverage

`cargo test --manifest-path src-tauri/Cargo.toml` covers workspace search, Git status, Kilo model filtering, local-agent reuse, and canonical workspace path checks. The boundary helper has an outside sibling path case; a Unix symlink escape test is compiled only on Unix and is not exercised by Windows CI today. There is no end-to-end OpenHands mock-server, UI automation, or clean-install suite yet.

`npm run build` runs TypeScript type checking and the Vite production build. It is not a UI behavior test.
