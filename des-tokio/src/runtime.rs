use std::cell::RefCell;
use std::sync::{Arc, Mutex as StdMutex};

use descartes_core::async_runtime::{
    self, DesRuntime, DesRuntimeHandle, DesRuntimeLocalHandle, RuntimeEvent,
};
use descartes_core::{Key, SchedulerHandle, SimTime, Simulation};

use crate::concurrency::{ConcurrencyEvent, ConcurrencyRecorder};
use crate::sync::mutex::{FifoMutexWaiterPolicy, MutexWaiterPolicy, WaiterInfo};

#[derive(Clone)]
struct Installed {
    handle: DesRuntimeHandle,
    local_handle: DesRuntimeLocalHandle,
    scheduler: SchedulerHandle,

    mutex_policy: Arc<StdMutex<Box<dyn MutexWaiterPolicy>>>,
    concurrency_recorder: Option<Arc<dyn ConcurrencyRecorder>>,
}

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Configuration for `descartes_tokio` facilities installed into a simulation.
#[derive(Default)]
pub struct TokioInstallConfig {
    pub mutex_policy: Option<Box<dyn MutexWaiterPolicy>>,
    pub concurrency_recorder: Option<Arc<dyn ConcurrencyRecorder>>,
}

/// Install the DES async runtime into the simulation and make it the
/// current runtime for `descartes_tokio::spawn`.
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
    install_with_tokio(sim, TokioInstallConfig::default(), configure)
}

/// Install the runtime with additional tokio-level configuration.
///
/// This is the primary hook for installing concurrency exploration tooling
/// (mutex scheduling policies, event recorders, etc.).
///
/// # Panics
///
/// Panics if a runtime is already installed in this thread.
pub fn install_with_tokio(
    sim: &mut Simulation,
    tokio_cfg: TokioInstallConfig,
    configure: impl FnOnce(&mut DesRuntime),
) -> Key<RuntimeEvent> {
    let mut runtime = DesRuntime::new();
    configure(&mut runtime);

    let scheduler = sim.scheduler_handle();
    let installed = async_runtime::install(sim, runtime);

    INSTALLED.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_some() {
            panic!("descartes_tokio runtime already installed");
        }
        let mutex_policy: Box<dyn MutexWaiterPolicy> = tokio_cfg
            .mutex_policy
            .unwrap_or_else(|| Box::new(FifoMutexWaiterPolicy));

        *slot = Some(Installed {
            handle: installed.handle,
            local_handle: installed.local_handle,
            scheduler,
            mutex_policy: Arc::new(StdMutex::new(mutex_policy)),
            concurrency_recorder: tokio_cfg.concurrency_recorder,
        });
    });

    installed.runtime_key
}

pub(crate) fn with_scheduler<R>(f: impl FnOnce(&SchedulerHandle) -> R) -> R {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let installed = slot.as_ref().expect(
            "descartes_tokio runtime not installed. Call descartes_tokio::runtime::install(&mut Simulation) first",
        );
        f(&installed.scheduler)
    })
}

pub(crate) fn with_handle<R>(f: impl FnOnce(&DesRuntimeHandle) -> R) -> R {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let installed = slot.as_ref().expect(
            "descartes_tokio runtime not installed. Call descartes_tokio::runtime::install(&mut Simulation) first",
        );
        f(&installed.handle)
    })
}

pub(crate) fn with_local_handle<R>(f: impl FnOnce(&DesRuntimeLocalHandle) -> R) -> R {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let installed = slot.as_ref().expect(
            "descartes_tokio runtime not installed. Call descartes_tokio::runtime::install(&mut Simulation) first",
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

/// Scheduler-level diagnostics accessible from within simulated tasks
/// 
/// Uses `try_lock` so it is safe to call from within event processing
/// (where the scheduler lock is already held). Returns `None` for any counter
/// that could not be read without blocking.
pub fn scheduler_diagnostics() -> (Option<usize>, Option<usize>, Option<usize>) {
    with_scheduler(|s| {
        (
            s.try_event_count(),
            s.try_completed_task_count(),
            s.try_cancelled_task_count()
        )
    })
}

/// Runtime-level diagnostics accessible from within simulated tasks
/// 
/// Reads thread-local counters updated by `DesRuntime::process_event`, so
/// this is lock-free and always succeeds.
pub fn runtime_diagnostics() -> descartes_core::async_runtime::RuntimeDiagnostics {
    descartes_core::async_runtime::runtime_diagnostics()
}

pub(crate) fn current_time_nanos() -> Option<u64> {
    let t = async_runtime::current_sim_time()?;
    let nanos: u128 = t.as_duration().as_nanos();
    Some(nanos.try_into().unwrap_or(u64::MAX))
}

pub(crate) fn record_concurrency_event(event: ConcurrencyEvent) {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let Some(installed) = slot.as_ref() else {
            return;
        };

        if let Some(recorder) = installed.concurrency_recorder.as_ref() {
            recorder.record(event);
        }
    })
}

pub(crate) fn choose_mutex_waiter(time: SimTime, waiters: &[WaiterInfo]) -> usize {
    INSTALLED.with(|cell| {
        let slot = cell.borrow();
        let installed = slot.as_ref().expect(
            "descartes_tokio runtime not installed. Call descartes_tokio::runtime::install(&mut Simulation) first",
        );

        let mut policy = installed.mutex_policy.lock().unwrap();
        policy.choose(time, waiters)
    })
}
