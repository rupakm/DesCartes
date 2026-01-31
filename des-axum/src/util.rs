use des_components::transport::TransportEvent;
use des_core::{Key, SchedulerHandle, SimTime};

pub(crate) fn schedule_transport(
    scheduler: &SchedulerHandle,
    transport_key: Key<TransportEvent>,
    event: TransportEvent,
) {
    if des_core::scheduler::in_scheduler_context() {
        des_core::defer_wake(transport_key, event);
    } else {
        scheduler.schedule(SimTime::zero(), transport_key, event);
    }
}

pub(crate) fn now(scheduler: &SchedulerHandle) -> SimTime {
    if des_core::scheduler::in_scheduler_context() {
        des_core::scheduler::current_time().unwrap_or(SimTime::zero())
    } else {
        scheduler.time()
    }
}
