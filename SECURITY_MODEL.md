# Flux Hands security model

## Current controls

- Provider API keys are accessed through the Windows credential store or Android secure storage, not browser `localStorage`.
- Local model serving is bound to `127.0.0.1`.
- OpenHands conversations now request confirmation before tool actions. The UI shows the tool name, risk value and serialized action, then lets the user approve or reject the pending batch. Existing conversations receive the policy update before new work is sent.
- The native layer records the active project from the OS folder picker, persists that choice, and migrates the earlier saved path once. File listing, reading, writing, search, Git status, model downloads and local Agent Server startup use that native active root rather than trusting a root supplied with each UI request. Targets are canonicalized and paths outside the root (including resolved symlink targets) are rejected. The editor save command only updates an existing file.
- Hugging Face download validates repository and relative GGUF path and avoids overwriting an existing model file.
- Updater downloads use Tauri's signed updater mechanism; publishing requires CI signing secrets.

## Trust boundaries and limitations

- A user-selected project is trusted input. The app is not an OS sandbox. The OpenHands local runtime and terminal tool execute with the current Windows user's operating-system permissions. Tool confirmation is a user-control layer, not a substitute for a restricted OS account/container.
- Confirmation decisions apply to all pending OpenHands actions in that response batch. Inspect every displayed action before approving. A server that does not support the v1.26 confirmation API must fail the workflow; the app must not fall back to automatic approval.
- The app's native file commands enforce a project root, but this does not constrain OpenHands' independent runtime tools. Runtime workspace isolation remains a separate P0 verification.
- Android stores its endpoint and remote workspace path locally and sends task content to the configured Agent Server/provider. Android does not execute local GGUF models.
- Kilo Free is an upstream service with changing availability and rate limits. Remote providers may charge for requests and retain submitted prompts under their terms. Local inference keeps model requests on the machine, but agent runtime downloads and Hugging Face catalog/model downloads still use the network.
- `src/main.tsx` currently configures Tauri CSP as `null`; review and tighten the content security policy before broad public release.
- A local debug APK or unsigned local installer is not an official production artifact. A production release needs authenticated GitHub publication and the configured updater signing secrets.

## Required security checks before public-ready status

1. Exercise approve and reject against an OpenHands 1.26 Agent Server with a tool-capable mock model; verify no tool executes before approval.
2. Verify project file operations reject `..`, absolute paths on another drive, symlinks/junctions to outside paths, and concurrent root changes.
3. Verify the OpenHands runtime cannot read/write arbitrary files or run destructive commands without an approval; document that execution still uses the user's OS identity.
4. Test secret storage on clean Windows and Android devices and inspect logs/UI for accidental credential disclosure.
5. Test signed update verification and interrupted-update recovery from a published release.
