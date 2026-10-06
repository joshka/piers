//! Wasmtime runtime support for loading guest components.
//!
//! This module owns the Wasmtime-specific setup used by the native harness:
//! component-model configuration, WASI Preview 2 imports, and instantiation of
//! generated bindings. Keeping it narrow makes the reload policy in
//! `harness.rs` easier to read without losing the runtime details.

use std::path::Path;

use anyhow::{Result, anyhow};
use tracing::debug;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::guest_component::Guest;

/// Host state made available to the Wasm guest through WASI Preview 2.
///
/// The current guest does not use filesystem, network, or clock capabilities,
/// so the WASI context is empty. It still needs a resource table because
/// Wasmtime's Preview 2 integration expects one for component instances.
pub struct HostState {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl HostState {
    fn new() -> Self {
        Self {
            ctx: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

/// Creates a Wasmtime engine configured for component-model guests.
///
/// The engine is reused across reloads so component compilation and runtime
/// configuration stay host-owned instead of becoming part of each guest
/// generation.
pub fn create_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    Engine::new(&config).map_err(|error| anyhow!("create Wasmtime engine: {error:#}"))
}

/// Instantiates a guest component from an already-built Wasm artifact.
///
/// Each call returns a fresh [`Store`] and guest binding. Guest memory and
/// thread-local state therefore belong to that instance and must be moved across
/// reloads through the guest's explicit snapshot and restore exports.
pub fn instantiate_artifact(engine: &Engine, artifact: &Path) -> Result<(Store<HostState>, Guest)> {
    debug!(artifact = %artifact.display(), "loading guest component");
    let component = Component::from_file(engine, artifact)
        .map_err(|error| anyhow!("load guest component {}: {error:#}", artifact.display()))?;
    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .map_err(|error| anyhow!("add WASI Preview 2 imports to linker: {error:#}"))?;
    let mut store = Store::new(engine, HostState::new());
    let guest = Guest::instantiate(&mut store, &component, &linker).map_err(|error| {
        anyhow!(
            "instantiate guest component {}: {error:#}",
            artifact.display()
        )
    })?;
    Ok((store, guest))
}
