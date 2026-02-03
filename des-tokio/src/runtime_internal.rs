use des_core::async_runtime::RuntimeEvent;
use des_core::{scheduler, Key};

use crate::runtime;

pub(crate) fn ensure_polled(runtime_key: Key<RuntimeEvent>) {
    if scheduler::in_scheduler_context() {
        // In event processing, it is safe and efficient to defer_wake.
        des_core::defer_wake(runtime_key, RuntimeEvent::Poll);
    } else {
        // Outside scheduler context (setup-time), schedule a poll via SchedulerHandle.
        runtime::with_scheduler(|sched| {
            sched.schedule_now(runtime_key, RuntimeEvent::Poll);
        });
    }
}
