//! Filesystem staging and promotion for generated guest updates.
//!
//! Generated guest source is never written directly over the live source. A
//! reload attempt first gets its own minimal workspace under `.piers/staged`,
//! builds there, and only promotes source plus artifact after the harness has
//! instantiated and smoke-tested the staged component.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use tracing::{debug, info};

/// Relative path to the guest source file inside the workspace.
pub const GUEST_SOURCE: &str = "guest/src/lib.rs";

/// Relative path to the guest Wasm artifact produced by Cargo.
pub const GUEST_ARTIFACT: &str = "target/wasm32-wasip2/debug/piers_guest.wasm";

const STAGING_DIR: &str = ".piers/staged";

/// A generated guest candidate staged outside the live source tree.
///
/// The staged root is disposable. The live workspace observes only the source
/// and artifact promoted after a successful build and smoke test.
pub struct StagedGuest {
    /// Root of the temporary workspace used for this reload attempt.
    pub root: PathBuf,
    /// Built Wasm artifact for the staged guest.
    pub artifact: PathBuf,
}

impl StagedGuest {
    /// Creates a staged workspace containing the generated guest source.
    ///
    /// The staged workspace copies the host `src/` directory as well as the WIT
    /// package because the workspace manifest points at both crates. The copy is
    /// intentionally small, but complete enough for Cargo to build the guest
    /// crate as if it were in the live checkout.
    pub fn create(root: &Path, source: &str, generation: u64) -> Result<Self> {
        validate_guest_source(source)?;

        let attempt = format!("gen-{generation}-{}", timestamp_millis()?);
        let staged_root = root.join(STAGING_DIR).join(attempt);
        let staged_guest_src = staged_root.join(GUEST_SOURCE);

        info!(
            generation,
            staged_root = %staged_root.display(),
            "staging guest source"
        );

        copy_file(root, &staged_root, "Cargo.toml")?;
        copy_file(root, &staged_root, "Cargo.lock")?;
        copy_dir(root.join("src"), staged_root.join("src"))?;
        copy_file(root, &staged_root, "guest/Cargo.toml")?;
        copy_dir(root.join("wit"), staged_root.join("wit"))?;

        let staged_guest_src_parent = staged_guest_src
            .parent()
            .ok_or_else(|| anyhow!("staged guest source has no parent"))?;
        fs_err::create_dir_all(staged_guest_src_parent)
            .with_context(|| format!("create {}", staged_guest_src_parent.display()))?;
        fs_err::write(&staged_guest_src, source)
            .with_context(|| format!("write staged guest source {}", staged_guest_src.display()))?;

        Ok(Self {
            artifact: staged_root.join(GUEST_ARTIFACT),
            root: staged_root,
        })
    }
}

/// Ensures that the live guest artifact exists before the harness starts.
pub fn ensure_guest_artifact(root: &Path) -> Result<()> {
    if root.join(GUEST_ARTIFACT).exists() {
        return Ok(());
    }
    build_guest_in(root)
}

/// Builds the guest crate in the provided workspace root.
///
/// This shells out to Cargo rather than linking through Cargo internals. That
/// keeps the PoC small and makes the build contract visible in error messages.
pub fn build_guest_in(root: &Path) -> Result<()> {
    info!(root = %root.display(), "building guest");
    let output = Command::new("cargo")
        .args(["build", "-p", "piers-guest", "--target", "wasm32-wasip2"])
        .current_dir(root)
        .output()
        .with_context(|| format!("spawn cargo build for guest in {}", root.display()))?;

    if output.status.success() {
        debug!(root = %root.display(), "guest build succeeded");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "guest build failed in {}\nstatus: {}\nstderr:\n{}",
        root.display(),
        output.status,
        stderr.trim_end()
    );
}

/// Checks whether generated source still looks like the expected guest component.
///
/// This is a structural sanity check, not a Rust parser or sandbox. The build
/// and smoke test remain the authoritative validation steps.
pub fn validate_guest_source(source: &str) -> Result<()> {
    if !source.contains("wit_bindgen::generate!") || !source.contains("export!(PiersGuest);") {
        bail!("guest update does not look like a piers guest component");
    }
    Ok(())
}

/// Promotes generated source into the live guest source path.
///
/// The caller is responsible for building and smoke-testing the same source in
/// a staging workspace before calling this function.
pub fn promote_guest_source(root: &Path, source: &str) -> Result<()> {
    validate_guest_source(source)?;

    let path = root.join(GUEST_SOURCE);
    ensure_path_inside_repo(root, &path)?;
    fs_err::write(&path, source)
        .with_context(|| format!("write live guest source {}", path.display()))
}

/// Promotes a staged Wasm artifact into the live artifact path.
///
/// The artifact is copied rather than moved so the staging directory remains
/// available for inspection after a reload attempt.
pub fn promote_guest_artifact(root: &Path, staged_artifact: &Path) -> Result<()> {
    let live_artifact = root.join(GUEST_ARTIFACT);
    let parent = live_artifact
        .parent()
        .ok_or_else(|| anyhow!("guest artifact has no parent"))?;
    fs_err::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    fs_err::copy(staged_artifact, &live_artifact).with_context(|| {
        format!(
            "copy staged artifact {} to {}",
            staged_artifact.display(),
            live_artifact.display()
        )
    })?;
    Ok(())
}

/// Refuses writes whose parent directory resolves outside the workspace root.
fn ensure_path_inside_repo(root: &Path, path: &Path) -> Result<()> {
    let canonical_root = root.canonicalize().context("canonicalize repo root")?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", path.display()))?;
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("canonicalize {}", parent.display()))?;
    if !canonical_parent.starts_with(&canonical_root) {
        bail!("refusing to write outside repo: {}", path.display());
    }
    Ok(())
}

/// Copies one workspace-relative file into a staged workspace.
fn copy_file(source_root: &Path, target_root: &Path, relative: &str) -> Result<()> {
    let source = source_root.join(relative);
    let target = target_root.join(relative);
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", target.display()))?;
    fs_err::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    fs_err::copy(&source, &target)
        .with_context(|| format!("copy {} to {}", source.display(), target.display()))?;
    Ok(())
}

/// Recursively copies regular files from one directory to another.
fn copy_dir(source: PathBuf, target: PathBuf) -> Result<()> {
    fs_err::create_dir_all(&target).with_context(|| format!("create {}", target.display()))?;
    for entry in fs_err::read_dir(&source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", source.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type for {}", entry.path().display()))?;
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(entry.path(), target_path)?;
        } else if file_type.is_file() {
            fs_err::copy(entry.path(), &target_path).with_context(|| {
                format!(
                    "copy {} to {}",
                    entry.path().display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Returns a coarse timestamp for human-readable staging directory names.
fn timestamp_millis() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_millis())
}
