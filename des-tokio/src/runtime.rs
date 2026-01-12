use std::cell::RefCell;

use des_core::async_runtime::{
    self, DesRuntime, DesRuntimeHandle, DesRuntimeLocalHandle, RuntimeEvent,
};
use des_core::{Key, SchedulerHandle, Simulation};

#[derive(Clone)]
struct Installed {
    handle: DesRuntimeHandle,
    local_handle: DesRuntimeLocalHandle,
    scheduler: SchedulerHandle,
}

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Install the DES async runtime into the simulation and make it the
/// current runtime for `des_tokio::spawn`.
///
/// # Panics
///
/// Panics if a runtime is already installed in this thread.
pub fn install(sim: &mut Simulation) -> Key<RuntimeEvent> {
    install_with(sim, |_| {})
}

/// Install the DES async runtime into the simulation and allow configuring it
/// before installation.
///
/// This is the primary opt-in hook for experimentation tooling (e.g. installing
/// a custom ready-task scheduling policy). The default `install` behavior remains
/// deterministic FIFO.
///
/// # Panics
///
/// Panics if a runtime is already installed in this thread.
pub fn install_with(
    sim: &mut Simulation,
    configure: impl FnOnce(&mut DesRuntime),
) -> Key<RuntimeEvent> {
    let mut runtime = DesRuntime::new();
    configure(&mut runtime);

    let scheduler = sim.scheduler_handle();
    let installed = async_runtime::install(sim, runtime);

    INSTALLED.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_some() {
            panic!("des_tokio runtime already installed");
        }
        *slot = Some(Installed {
            handle: installed.handle,
            local_handle: installed.local_handle,
            scheduler,
        });
    });

    installed.runtime_key
}

pub(crate) fn with_scheduler<R>(f: impl FnOnce(&SchedulerHandle) -> R) -> R {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let installed = slot.as_ref().expect(
            "des_tokio runtime not installed. Call des_tokio::runtime::install(&mut Simulation) first",
        );
        f(&installed.scheduler)
    })
}

pub(crate) fn with_handle<R>(f: impl FnOnce(&DesRuntimeHandle) -> R) -> R {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let installed = slot.as_ref().expect(
            "des_tokio runtime not installed. Call des_tokio::runtime::install(&mut Simulation) first",
        );
        f(&installed.handle)
    })
}

pub(crate) fn with_local_handle<R>(f: impl FnOnce(&DesRuntimeLocalHandle) -> R) -> R {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let installed = slot.as_ref().expect(
            "des_tokio runtime not installed. Call des_tokio::runtime::install(&mut Simulation) first",
        );
        f(&installed.local_handle)
    })
}

pub(crate) fn installed_handle() -> DesRuntimeHandle {
    with_handle(|h| h.clone())
}

pub(crate) fn installed_local_handle() -> DesRuntimeLocalHandle {
    with_local_handle(|h| h.clone())
}
