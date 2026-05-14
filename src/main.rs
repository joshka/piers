use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    path: "wit",
    world: "guest",
});

const GUEST_SOURCE: &str = "guest/src/lib.rs";
const GUEST_ARTIFACT: &str = "target/wasm32-wasip2/debug/piers_guest.wasm";

fn main() -> Result<()> {
    let root = std::env::current_dir().context("get current directory")?;
    let mut harness = Harness::load(root)?;

    println!("piers harness");
    println!("commands: :evolve <spec>, :reload, :status, :quit");

    let stdin = io::stdin();
    loop {
        print!("piers> ");
        io::stdout().flush().context("flush prompt")?;

        let mut line = String::new();
        if stdin.read_line(&mut line).context("read input")? == 0 {
            break;
        }

        let line = line.trim_end();
        if line == ":quit" {
            break;
        } else if line == ":status" {
            println!("{}", harness.status());
        } else if line == ":reload" {
            harness.reload()?;
            println!("reloaded generation {}", harness.generation);
        } else if let Some(spec) = line.strip_prefix(":evolve") {
            harness.evolve(spec.trim())?;
        } else if line.is_empty() {
            continue;
        } else {
            println!("{}", harness.handle(line)?);
        }
    }

    Ok(())
}

struct Harness {
    root: PathBuf,
    engine: Engine,
    store: Store<HostState>,
    guest: Guest,
    generation: u64,
    last_rebuild: String,
}

impl Harness {
    fn load(root: PathBuf) -> Result<Self> {
        ensure_guest_artifact(&root)?;

        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = wt(Engine::new(&config), "create Wasmtime engine")?;
        let (store, guest) = instantiate(&engine, &root)?;

        Ok(Self {
            root,
            engine,
            store,
            guest,
            generation: 0,
            last_rebuild: "initial build ok".to_string(),
        })
    }

    fn handle(&mut self, input: &str) -> Result<String> {
        self.guest
            .call_handle(&mut self.store, input)
            .map_err(|error| anyhow!("call guest handle: {error:#}"))
    }

    fn evolve(&mut self, spec: &str) -> Result<()> {
        if spec.is_empty() {
            bail!("usage: :evolve <spec>");
        }

        let next_source = self
            .guest
            .call_propose_update(&mut self.store, spec)
            .map_err(|error| anyhow!("ask guest for update: {error:#}"))?;
        write_guest_source(&self.root, &next_source)?;

        match self.reload() {
            Ok(()) => {
                println!("evolved and reloaded generation {}", self.generation);
                Ok(())
            }
            Err(error) => {
                self.last_rebuild = format!("failed: {error:#}");
                Err(error)
            }
        }
    }

    fn reload(&mut self) -> Result<()> {
        let snapshot = self
            .guest
            .call_snapshot(&mut self.store)
            .map_err(|error| anyhow!("snapshot current guest: {error:#}"))?;

        build_guest(&self.root)?;
        let (mut next_store, next_guest) = instantiate(&self.engine, &self.root)?;
        next_guest
            .call_restore(&mut next_store, &snapshot)
            .map_err(|error| anyhow!("restore guest snapshot: {error:#}"))?;

        self.store = next_store;
        self.guest = next_guest;
        self.generation += 1;
        self.last_rebuild = "ok".to_string();
        Ok(())
    }

    fn status(&self) -> String {
        format!(
            "generation: {}\nartifact: {}\nlast rebuild: {}",
            self.generation, GUEST_ARTIFACT, self.last_rebuild
        )
    }
}

struct HostState {
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

fn instantiate(engine: &Engine, root: &Path) -> Result<(Store<HostState>, Guest)> {
    let component = Component::from_file(engine, root.join(GUEST_ARTIFACT))
        .map_err(|error| anyhow!("load {GUEST_ARTIFACT}: {error:#}"))?;
    let mut linker = Linker::new(engine);
    wt(
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker),
        "add WASI Preview 2 imports to linker",
    )?;
    let mut store = Store::new(engine, HostState::new());
    let guest = Guest::instantiate(&mut store, &component, &linker)
        .map_err(|error| anyhow!("instantiate guest: {error:#}"))?;
    Ok((store, guest))
}

fn wt<T>(result: std::result::Result<T, wasmtime::Error>, context: &str) -> Result<T> {
    result.map_err(|error| anyhow!("{context}: {error:#}"))
}

fn ensure_guest_artifact(root: &Path) -> Result<()> {
    if root.join(GUEST_ARTIFACT).exists() {
        return Ok(());
    }
    build_guest(root)
}

fn build_guest(root: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args(["build", "-p", "piers-guest", "--target", "wasm32-wasip2"])
        .current_dir(root)
        .output()
        .context("spawn cargo build for guest")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("guest build failed\n{stderr}");
}

fn write_guest_source(root: &Path, source: &str) -> Result<()> {
    if !source.contains("wit_bindgen::generate!") || !source.contains("export!(PiersGuest);") {
        bail!("guest update does not look like a piers guest component");
    }

    let path = root.join(GUEST_SOURCE);
    let canonical_root = root.canonicalize().context("canonicalize repo root")?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("guest source has no parent"))?;
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("canonicalize {}", parent.display()))?;
    if !canonical_parent.starts_with(&canonical_root) {
        bail!("refusing to write outside repo");
    }

    fs::write(&path, source).with_context(|| format!("write {}", path.display()))
}
