//! Unified DES waker utilities.
//!
//! This module provides a common waker implementation for async code running
//! within the discrete event simulation. It uses the `defer_wake` mechanism
//! to schedule events when futures need to be polled.
//!
//! # Usage
//!
//! ```rust,ignore
//! use des_core::waker::create_des_waker;
//!
//! // Create a waker that schedules a poll event when woken
//! let waker = create_des_waker(component_key, MyEvent::Poll { task_id });
//! let mut cx = Context::from_waker(&waker);
//! ```

use std::fmt::Debug;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable, Waker};

use crate::{defer_wake, Key};

/// Arc-wrapped waker data for efficient cloning.
struct WakerDataArc {
    schedule_fn: Box<dyn Fn() + Send + Sync>,
}

impl WakerDataArc {
    fn new<E: Debug + Clone + Send + Sync + 'static>(key: Key<E>, event: E) -> Arc<Self> {
        Arc::new(Self {
            schedule_fn: Box::new(move || {
                defer_wake(key, event.clone());
            }),
        })
    }

    fn wake(&self) {
        (self.schedule_fn)();
    }
}

/// VTable for Arc-based DES waker.
static DES_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    des_waker_clone,
    des_waker_wake,
    des_waker_wake_by_ref,
    des_waker_drop,
);

unsafe fn des_waker_clone(data: *const ()) -> RawWaker {
    Arc::increment_strong_count(data as *const WakerDataArc);
    RawWaker::new(data, &DES_WAKER_VTABLE)
}

unsafe fn des_waker_wake(data: *const ()) {
    let arc = Arc::from_raw(data as *const WakerDataArc);
    arc.wake();
    // Arc is dropped here, decrementing ref count
}

unsafe fn des_waker_wake_by_ref(data: *const ()) {
    let waker_data = &*(data as *const WakerDataArc);
    waker_data.wake();
}

unsafe fn des_waker_drop(data: *const ()) {
    drop(Arc::from_raw(data as *const WakerDataArc));
}

/// Create a DES-aware waker that schedules an event when woken.
///
/// When the waker is triggered (by calling `wake()` or `wake_by_ref()`),
/// it uses `defer_wake` to schedule the provided event to the component.
/// This only works inside scheduler context during single-threaded event processing.
///
/// # Arguments
///
/// * `key` - The component key to schedule the event to
/// * `event` - The event to schedule (must be Clone + Debug + Send + Sync + 'static)
///
/// # Example
///
/// ```rust,ignore
/// let waker = create_des_waker(runtime_key, RuntimeEvent::Wake { task_id });
/// let mut cx = Context::from_waker(&waker);
/// let _ = future.poll(&mut cx);
/// ```
pub fn create_des_waker<E: Debug + Clone + Send + Sync + 'static>(key: Key<E>, event: E) -> Waker {
    let arc = WakerDataArc::new(key, event);
    let raw_waker = RawWaker::new(Arc::into_raw(arc) as *const (), &DES_WAKER_VTABLE);
    unsafe { Waker::from_raw(raw_waker) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestEvent(usize);

    #[test]
    fn test_waker_clone() {
        // Create a waker (we can't actually test defer_wake without a scheduler context,
        // but we can test that the waker can be cloned without panicking)
        let key: Key<TestEvent> = Key::new_with_id(uuid::Uuid::now_v7());
        let waker = create_des_waker(key, TestEvent(42));

        // Clone should work
        let waker2 = waker.clone();
        let waker3 = waker2.clone();

        // Drop in various orders
        drop(waker);
        drop(waker3);
        drop(waker2);
    }

    #[test]
    fn test_waker_will_wake() {
        let key: Key<TestEvent> = Key::new_with_id(uuid::Uuid::now_v7());
        let waker1 = create_des_waker(key, TestEvent(1));
        let waker2 = waker1.clone();

        // Same waker should report will_wake
        assert!(waker1.will_wake(&waker2));
    }
}
