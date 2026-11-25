use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let files = [
        "config/gateway.json",
        "config/local-model.json",
        "components/gateway/server.py",
        "components/lmstudio-client/load-model.bundle.mjs",
        "scripts/runtime.py",
        "scripts/manage.py",
        "scripts/qwen-work.py",
        "scripts/download-model.py",
    ];
    let mut generated = "pub const BACKEND: &[(&str, &[u8])] = &[\n".to_string();
    for relative in files {
        let path = root.join("backend").join(relative);
        assert!(
            path.exists(),
            "Missing backend asset {}. Run npm run bundle in backend/components/lmstudio-client first.",
            path.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
        generated += &format!(
            "({relative:?}, include_bytes!({:?})),\n",
            path.to_string_lossy()
        );
    }
    generated += "];\n";
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("backend_assets.rs"),
        generated,
    )
    .unwrap();
}
