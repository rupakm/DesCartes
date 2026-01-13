use std::sync::atomic::{AtomicU64 as StdAtomicU64, Ordering};

use des_core::async_runtime;

use crate::concurrency::ConcurrencyEvent;

#[derive(Debug)]
pub struct AtomicU64 {
    site_id: u64,
    inner: StdAtomicU64,
}

impl AtomicU64 {
    pub fn new(site_id: u64, value: u64) -> Self {
        Self {
            site_id,
            inner: StdAtomicU64::new(value),
        }
    }

    /// Construct an atomic with a stable name-based id.
    pub fn new_named(name: &'static str, value: u64) -> Self {
        let site_id =
            des_core::randomness::runtime_site_id("des_tokio::sync::atomic::AtomicU64", name);
        Self::new(site_id, value)
    }

    pub fn site_id(&self) -> u64 {
        self.site_id
    }

    pub fn load(&self, ordering: Ordering) -> u64 {
        let value = self.inner.load(ordering);
        crate::runtime::record_concurrency_event(ConcurrencyEvent::AtomicLoad {
            site_id: self.site_id,
            task_id: current_task_id_u64(),
            time_nanos: crate::runtime::current_time_nanos(),
            ordering,
            value,
        });
        value
    }

    pub fn store(&self, value: u64, ordering: Ordering) {
        self.inner.store(value, ordering);
        crate::runtime::record_concurrency_event(ConcurrencyEvent::AtomicStore {
            site_id: self.site_id,
            task_id: current_task_id_u64(),
            time_nanos: crate::runtime::current_time_nanos(),
            ordering,
            value,
        });
    }

    pub fn fetch_add(&self, delta: u64, ordering: Ordering) -> u64 {
        let prev = self.inner.fetch_add(delta, ordering);
        let next = prev.wrapping_add(delta);
        crate::runtime::record_concurrency_event(ConcurrencyEvent::AtomicFetchAdd {
            site_id: self.site_id,
            task_id: current_task_id_u64(),
            time_nanos: crate::runtime::current_time_nanos(),
            ordering,
            prev,
            next,
            delta,
        });
        prev
    }

    pub fn compare_exchange(
        &self,
        expected: u64,
        new: u64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<u64, u64> {
        let res = self.inner.compare_exchange(expected, new, success, failure);
        let current = match res {
            Ok(v) => v,
            Err(v) => v,
        };

        crate::runtime::record_concurrency_event(ConcurrencyEvent::AtomicCompareExchange {
            site_id: self.site_id,
            task_id: current_task_id_u64(),
            time_nanos: crate::runtime::current_time_nanos(),
            success_order: success,
            failure_order: failure,
            current,
            expected,
            new,
            succeeded: res.is_ok(),
        });

        res
    }
}

fn current_task_id_u64() -> u64 {
    async_runtime::current_task_id()
        .map(|t| t.0)
        .unwrap_or_else(|| panic!("des_tokio::sync::atomic used outside async runtime polling"))
}
