use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};

use super::semaphore::{Semaphore, SemaphorePermit};

const MAX_READS: usize = (u32::MAX >> 3) as usize;

pub struct RwLock<T: ?Sized> {
    max_readers: usize,
    sem: Semaphore,
    value: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            max_readers: MAX_READS,
            sem: Semaphore::new(MAX_READS),
            value: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> RwLock<T> {
    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        let permit = self
            .sem
            .acquire()
            .await
            .expect("rwlock semaphore must not be closed");
        RwLockReadGuard {
            lock: self,
            _permit: permit,
        }
    }

    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        let permit = self
            .sem
            .acquire_many(self.max_readers)
            .await
            .expect("rwlock semaphore must not be closed");
        RwLockWriteGuard {
            lock: self,
            _permit: permit,
        }
    }

    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let permit = self.sem.try_acquire().ok()?;
        Some(RwLockReadGuard {
            lock: self,
            _permit: permit,
        })
    }

    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        let permit = self.sem.try_acquire_many(self.max_readers).ok()?;
        Some(RwLockWriteGuard {
            lock: self,
            _permit: permit,
        })
    }
}

pub struct RwLockReadGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
    _permit: SemaphorePermit,
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: the semaphore permit guarantees shared access.
        unsafe { &*self.lock.value.get() }
    }
}

pub struct RwLockWriteGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
    _permit: SemaphorePermit,
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: the semaphore permit guarantees exclusive access.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: the semaphore permit guarantees exclusive access.
        unsafe { &mut *self.lock.value.get() }
    }
}
