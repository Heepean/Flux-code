# Release checklist

## Before tagging

- [ ] Update app and Android version codes consistently; add a changelog entry and release notes.
- [ ] `npm ci`, `npm run build`, `cargo fmt --check`, and `cargo test --manifest-path src-tauri/Cargo.toml` pass.
- [ ] Build Windows NSIS/MSI and Android APK from the exact tagged commit.
- [ ] Run local OpenHands/provider approve/reject integration scenario and test workspace boundary failures.
- [ ] Install and smoke-test Windows release on a clean supported system and APK on Android 15+ hardware.
- [ ] Verify release notes, known issues, artifact hashes, updater signature, manifest, APK signature and package metadata.
- [ ] Confirm the release workflow has required signing secrets and GitHub write credentials.

## After publication

- [ ] Confirm the GitHub tag and release point to the tested commit.
- [ ] Download each public artifact and verify its checksum/signature independently.
- [ ] Test update discovery and installation from the previous supported version.
- [ ] Confirm the release page includes both Windows installers and the intended Android APK, with an accurate debug/production label.
- [ ] Update README download links and record failures or skipped checks without claiming completion.
