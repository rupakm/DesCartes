use descartes_components::transport::TransportEvent;
use descartes_core::{Key, SchedulerHandle, SimTime};

pub(crate) fn schedule_transport(
    scheduler: &SchedulerHandle,
    transport_key: Key<TransportEvent>,
    event: TransportEvent,
) {
    if descartes_core::scheduler::in_scheduler_context() {
        descartes_core::defer_wake(transport_key, event);
    } else {
        scheduler.schedule(SimTime::zero(), transport_key, event);
    }
}

pub(crate) fn now(scheduler: &SchedulerHandle) -> SimTime {
    if descartes_core::scheduler::in_scheduler_context() {
        descartes_core::scheduler::current_time().unwrap_or(SimTime::zero())
    } else {
        scheduler.time()
    }
}
