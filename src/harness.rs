//! Host harness that owns guest lifecycle and reload promotion.
//!
//! The harness is the stable native side of the experiment. It owns the
//! Wasmtime engine, the active guest instance, the guest state snapshot flow,
//! and the rule that generated code must build and pass a smoke test before it
//! replaces the live source and artifact.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use tracing::{debug, error, info};
use wasmtime::{Engine, Store};

use crate::guest_component::Guest;
use crate::runtime::{HostState, create_engine, instantiate_artifact};
use crate::staging::{
    GUEST_ARTIFACT, StagedGuest, build_guest_in, ensure_guest_artifact, promote_guest_artifact,
    promote_guest_source,
};

/// Stable native host for the reloadable Piers guest component.
///
/// A `Harness` keeps host-owned runtime resources alive while guest behavior is
/// rebuilt and replaced. The guest owns only state it can serialize through the
/// WIT `snapshot` export, so reloads can move that state into a new component
/// instance without preserving guest memory.
pub struct Harness {
    root: PathBuf,
    engine: Engine,
    store: Store<HostState>,
    guest: Guest,
    generation: u64,
    last_rebuild: String,
}

impl Harness {
    /// Loads the current guest artifact and prepares the host runtime.
    ///
    /// If the artifact is missing, this builds `piers-guest` for the
    /// `wasm32-wasip2` target in `root` before instantiating it. The initial
    /// generation is `0`; later successful reloads increment it.
    pub fn load(root: PathBuf) -> Result<Self> {
        ensure_guest_artifact(&root)?;

        let engine = create_engine()?;
        let (store, guest) = instantiate_artifact(&engine, &root.join(GUEST_ARTIFACT))?;

        Ok(Self {
            root,
            engine,
            store,
            guest,
            generation: 0,
            last_rebuild: "initial build ok".to_string(),
        })
    }

    /// Returns the current guest generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Sends ordinary user input to the active guest.
    ///
    /// This call can mutate guest-local state. The current PoC guest increments
    /// a call counter, which lets reload tests verify that state handoff is
    /// working.
    pub fn handle(&mut self, input: &str) -> Result<String> {
        debug!(
            generation = self.generation,
            "handling input with active guest"
        );
        self.guest
            .call_handle(&mut self.store, input)
            .map_err(|error| anyhow!("call guest handle: {error:#}"))
    }

    /// Asks the active guest to generate replacement source and reloads it.
    ///
    /// The live source and artifact are not replaced until the generated source
    /// has been staged, built, restored from the active snapshot, and smoke
    /// tested. If any step fails, the active guest instance keeps running and
    /// `last_rebuild` records the failure.
    pub fn evolve(&mut self, spec: &str) -> Result<()> {
        if spec.is_empty() {
            bail!("usage: :evolve <spec>");
        }

        info!(generation = self.generation, "requesting guest evolution");
        let next_source = self
            .guest
            .call_propose_update(&mut self.store, spec)
            .map_err(|error| anyhow!("ask guest for update: {error:#}"))?;

        match self.reload_from_source(&next_source) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.last_rebuild = format!("failed: {error:#}");
                error!(error = %error, "guest evolution failed");
                Err(error)
            }
        }
    }

    /// Reloads from the current live guest source and artifact.
    ///
    /// This is a rebuild of the checked-in guest source, not a generated-source
    /// promotion. The current guest is snapshotted before the rebuild and the
    /// snapshot is restored into a fresh instance from the rebuilt artifact.
    pub fn reload(&mut self) -> Result<()> {
        let snapshot = self.snapshot_active_guest()?;

        build_guest_in(&self.root)?;
        let (mut next_store, next_guest) =
            instantiate_artifact(&self.engine, &self.root.join(GUEST_ARTIFACT))?;
        next_guest
            .call_restore(&mut next_store, &snapshot)
            .map_err(|error| anyhow!("restore guest snapshot: {error:#}"))?;

        self.store = next_store;
        self.guest = next_guest;
        self.generation += 1;
        self.last_rebuild = "ok".to_string();
        info!(generation = self.generation, "reloaded live guest artifact");
        Ok(())
    }

    /// Returns human-readable host status for the line REPL.
    pub fn status(&self) -> String {
        format!(
            "generation: {}\nartifact: {}\nlast rebuild: {}",
            self.generation, GUEST_ARTIFACT, self.last_rebuild
        )
    }

    /// Builds and promotes generated guest source as one reload attempt.
    ///
    /// Promotion is intentionally last. The staged artifact is instantiated
    /// twice: once for smoke testing and once for the new live guest. That keeps
    /// smoke-test mutations out of the promoted guest state.
    fn reload_from_source(&mut self, source: &str) -> Result<()> {
        let next_generation = self.generation + 1;
        let attempt = StagedGuest::create(&self.root, source, next_generation)?;
        build_guest_in(&attempt.root)?;

        let snapshot = self.snapshot_active_guest()?;

        self.smoke_test_staged_guest(&attempt, &snapshot)?;
        let (mut next_store, next_guest) = instantiate_artifact(&self.engine, &attempt.artifact)?;
        next_guest
            .call_restore(&mut next_store, &snapshot)
            .map_err(|error| anyhow!("restore promoted guest snapshot: {error:#}"))?;

        promote_guest_source(&self.root, source)?;
        promote_guest_artifact(&self.root, &attempt.artifact)?;

        self.store = next_store;
        self.guest = next_guest;
        self.generation = next_generation;
        self.last_rebuild = format!("ok: staged {}", attempt.root.display());
        info!(generation = self.generation, "promoted staged guest");
        Ok(())
    }

    /// Captures guest-owned state before a reload boundary is crossed.
    fn snapshot_active_guest(&mut self) -> Result<String> {
        self.guest
            .call_snapshot(&mut self.store)
            .map_err(|error| anyhow!("snapshot current guest: {error:#}"))
    }

    /// Verifies that a staged guest can accept the current state and run once.
    fn smoke_test_staged_guest(&self, attempt: &StagedGuest, snapshot: &str) -> Result<()> {
        let (mut smoke_store, smoke_guest) = instantiate_artifact(&self.engine, &attempt.artifact)?;
        smoke_guest
            .call_restore(&mut smoke_store, snapshot)
            .map_err(|error| anyhow!("restore staged guest snapshot: {error:#}"))?;
        smoke_guest
            .call_handle(&mut smoke_store, "__piers_reload_smoke__")
            .map_err(|error| anyhow!("smoke test staged guest: {error:#}"))?;
        debug!(artifact = %attempt.artifact.display(), "staged guest smoke test passed");
        Ok(())
    }
}
