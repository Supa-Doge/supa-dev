# Third-party components

Supa Dev does not redistribute the model, Qwen Code or LM Studio in its binary. The installer fetches them from their publishers; their original licenses and terms apply.

- Rust dependencies and versions: `Cargo.lock`; each crate retains its own license. Main UI: egui/eframe (MIT OR Apache-2.0).
- LM Studio JavaScript SDK: `@lmstudio/sdk` 2.1.0 (MIT), bundled for model loading. Dependency versions are in `backend/components/lmstudio-client/package-lock.json`. Copyright/license comments are retained inline in the generated bundle.
- Node.js comes from the official Qwen Code standalone package; its bundled notices remain with that installation.
- Windows Python embeddable distribution includes the Python license. Linux runtime is published by Astral's Python Build Standalone project and retains its bundled licenses.
- Model source and revision: `backend/config/local-model.json`; consult the model repository's license/model card before redistribution or use.
- Official LM Studio runtime is installed from `lmstudio.ai`; it remains governed by its publisher's terms.

Builds do not contain user API tokens or user conversation data.
