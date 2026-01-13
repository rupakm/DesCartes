use des_core::{scheduler, SchedulerHandle, SimTime};

pub(crate) fn now(scheduler_handle: &SchedulerHandle) -> SimTime {
    scheduler::current_time().unwrap_or_else(|| scheduler_handle.time())
}
