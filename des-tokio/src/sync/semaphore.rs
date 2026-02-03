use std::sync::Arc;

use super::batch_semaphore;

#[derive(Clone, Debug)]
pub struct Semaphore {
    inner: Arc<batch_semaphore::Semaphore>,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        Self {
            inner: Arc::new(batch_semaphore::Semaphore::new(permits)),
        }
    }

    pub fn available_permits(&self) -> usize {
        self.inner.available_permits()
    }

    pub fn add_permits(&self, permits: usize) {
        self.inner.add_permits(permits)
    }

    pub async fn acquire(&self) -> Result<SemaphorePermit, AcquireError> {
        self.acquire_many(1).await
    }

    pub async fn acquire_many(&self, permits: usize) -> Result<SemaphorePermit, AcquireError> {
        if permits == 0 {
            return Ok(SemaphorePermit {
                sem: self.inner.clone(),
                permits: 0,
            });
        }

        self.inner
            .acquire(permits)
            .await
            .map_err(|_| AcquireError)
            .map(|_| SemaphorePermit {
                sem: self.inner.clone(),
                permits,
            })
    }

    pub fn try_acquire(&self) -> Result<SemaphorePermit, TryAcquireError> {
        self.try_acquire_many(1)
    }

    pub fn try_acquire_many(&self, permits: usize) -> Result<SemaphorePermit, TryAcquireError> {
        self.inner.try_acquire(permits).map_err(|e| match e {
            batch_semaphore::TryAcquireError::Closed => TryAcquireError::Closed,
            batch_semaphore::TryAcquireError::NoPermits => TryAcquireError::NoPermits,
        })?;

        Ok(SemaphorePermit {
            sem: self.inner.clone(),
            permits,
        })
    }

    pub fn close(&self) {
        self.inner.close();
    }

    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquireError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryAcquireError {
    Closed,
    NoPermits,
}

#[derive(Debug)]
pub struct SemaphorePermit {
    sem: Arc<batch_semaphore::Semaphore>,
    permits: usize,
}

impl SemaphorePermit {
    pub fn permits(&self) -> usize {
        self.permits
    }

    pub fn forget(mut self) {
        self.permits = 0;
    }
}

impl Drop for SemaphorePermit {
    fn drop(&mut self) {
        if self.permits == 0 {
            return;
        }
        self.sem.add_permits(self.permits);
    }
}
