//! Exports the packaged Agave runtime version for fixture report metadata.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

const AGAVE_FEATURE_SET_DEPENDENCY: &str = "agave-feature-set";
const VERSION_ENV: &str = "HPSVM_AGAVE_FEATURE_SET_VERSION";

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let crate_manifest = manifest_dir.join("Cargo.toml");
    let workspace_manifest = manifest_dir.join("..").join("..").join("Cargo.toml");

    println!("cargo:rerun-if-changed={}", crate_manifest.display());
    if workspace_manifest.exists() {
        println!("cargo:rerun-if-changed={}", workspace_manifest.display());
    }

    let version = read_dependency_version(&crate_manifest)
        .or_else(|| read_dependency_version(&workspace_manifest))
        .unwrap_or_else(|| String::from("unknown"));

    println!("cargo:rustc-env={VERSION_ENV}={version}");
}

fn read_dependency_version(manifest_path: &Path) -> Option<String> {
    fs::read_to_string(manifest_path)
        .ok()
        .and_then(|manifest| extract_dependency_version(&manifest, AGAVE_FEATURE_SET_DEPENDENCY))
}

fn extract_dependency_version(manifest: &str, dependency_name: &str) -> Option<String> {
    let quoted_prefix = format!("{dependency_name} = \"");
    let inline_table_prefix = format!("{dependency_name} = {{");

    manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(|line| {
            line.strip_prefix(&quoted_prefix)
                .and_then(|value| value.split('"').next())
                .map(str::to_owned)
                .or_else(|| {
                    line.strip_prefix(&inline_table_prefix)
                        .and_then(|_| line.split("version = \"").nth(1))
                        .and_then(|value| value.split('"').next())
                        .map(str::to_owned)
                })
        })
}
