# Changelog

## v0.2.0

- Improved LSP setup for TypeScript/JavaScript, Python, Rust, Go, JSON, HTML, and CSS.
- Added editor actions for go-to-definition (F12) and document formatting (Shift+Alt+F).
- Improved Windows file URI handling and LSP definition links and formatting options.
- Added bounded, ranked project search with code context and exclusions for common generated folders and credential files.
- Added line-range reads for large text, DOCX, and PDF materials, with a bounded response size.
- Hardened the signed release workflow by validating the version tag and passing release assets between steps explicitly.
