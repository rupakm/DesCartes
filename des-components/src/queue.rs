//! Queue components for request buffering and ordering
//!
//! This module provides queue implementations with different ordering disciplines
//! (FIFO, priority-based) and capacity management.

use crate::error::QueueError;
use des_core::{SimTime, RequestAttempt};
use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, VecDeque};
use uuid::Uuid;

/// Item stored in a queue
///
/// QueueItem wraps a RequestAttempt along with metadata needed for queue management,
/// such as enqueue time, priority, and the original client that made the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    /// The request attempt being queued
    pub attempt: RequestAttempt,
    /// Simulation time when the item was enqueued
    pub enqueued_at: SimTime,
    /// Priority for priority-based queues (lower values = higher priority)
    pub priority: u32,
    /// Original client ID (stored as UUID for serialization)
    pub client_id: Option<Uuid>,
}

impl QueueItem {
    /// Create a new queue item with default priority
    pub fn new(attempt: RequestAttempt, enqueued_at: SimTime) -> Self {
        Self {
            attempt,
            enqueued_at,
            priority: 0,
            client_id: None,
        }
    }

    /// Create a new queue item with specified priority
    pub fn with_priority(attempt: RequestAttempt, enqueued_at: SimTime, priority: u32) -> Self {
        Self {
            attempt,
            enqueued_at,
            priority,
            client_id: None,
        }
    }

    /// Create a new queue item with client ID for proper response routing
    pub fn with_client_id(attempt: RequestAttempt, enqueued_at: SimTime, client_id: Uuid) -> Self {
        Self {
            attempt,
            enqueued_at,
            priority: 0,
            client_id: Some(client_id),
        }
    }

    /// Create a new queue item with priority and client ID
    pub fn with_priority_and_client_id(
        attempt: RequestAttempt, 
        enqueued_at: SimTime, 
        priority: u32,
        client_id: Uuid
    ) -> Self {
        Self {
            attempt,
            enqueued_at,
            priority,
            client_id: Some(client_id),
        }
    }

    /// Calculate how long this item has been in the queue
    pub fn queue_time(&self, current_time: SimTime) -> std::time::Duration {
        current_time.duration_since(self.enqueued_at)
    }
}

/// Core trait for queue implementations
///
/// This trait defines the interface for all queue types in the simulation framework.
/// Queues are used to buffer requests when resources are unavailable or to implement
/// specific ordering disciplines.
///
/// # Requirements
///
/// - 2.3: Provide Queue component with configurable capacity and queuing disciplines
/// - 3.1: Define trait-based interfaces for all Model_Component types
pub trait Queue: Send {
    /// Add an item to the queue
    ///
    /// # Arguments
    ///
    /// * `item` - The queue item to add
    ///
    /// # Errors
    ///
    /// Returns `QueueError::Full` if the queue is at capacity and cannot accept
    /// more items.
    fn enqueue(&mut self, item: QueueItem) -> Result<(), QueueError>;

    /// Remove and return the next item from the queue
    ///
    /// The specific item returned depends on the queue's ordering discipline:
    /// - FIFO queues return the oldest item
    /// - Priority queues return the highest priority item
    ///
    /// # Returns
    ///
    /// The next queue item, or `None` if the queue is empty.
    fn dequeue(&mut self) -> Option<QueueItem>;

    /// Get the current number of items in the queue
    fn len(&self) -> usize;

    /// Check if the queue is empty
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the maximum capacity of the queue
    ///
    /// # Returns
    ///
    /// The capacity limit, or `None` if the queue has unlimited capacity.
    fn capacity(&self) -> Option<usize>;

    /// Check if the queue is at capacity
    fn is_full(&self) -> bool {
        if let Some(cap) = self.capacity() {
            self.len() >= cap
        } else {
            false
        }
    }

    /// Peek at the next item without removing it
    ///
    /// # Returns
    ///
    /// A reference to the next queue item, or `None` if the queue is empty.
    fn peek(&self) -> Option<&QueueItem>;
}

/// First-In-First-Out (FIFO) queue implementation
///
/// FifoQueue processes items in the order they were added, implementing a standard
/// FIFO discipline. It supports optional capacity limits and tracks queue depth
/// metrics.
///
/// # Requirements
///
/// - 2.3: Provide Queue component with configurable capacity and queuing disciplines
///
/// # Examples
///
/// ```
/// use des_components::queue::{FifoQueue, QueueItem, Queue};
/// use des_components::{RequestAttempt, RequestAttemptId, RequestId};
/// use des_core::SimTime;
///
/// let mut queue = FifoQueue::new(Some(10)); // Capacity of 10
/// let attempt = RequestAttempt::new(
///     RequestAttemptId(1),
///     RequestId(1),
///     1,
///     SimTime::zero(),
///     vec![],
/// );
/// let item = QueueItem::new(attempt, SimTime::zero());
/// queue.enqueue(item).unwrap();
/// assert_eq!(queue.len(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct FifoQueue {
    /// Internal storage using VecDeque for efficient FIFO operations
    items: VecDeque<QueueItem>,
    /// Optional capacity limit
    capacity: Option<usize>,
    /// Total number of items enqueued (for metrics)
    total_enqueued: u64,
    /// Total number of items dequeued (for metrics)
    total_dequeued: u64,
}

impl FifoQueue {
    /// Create a new FIFO queue with optional capacity limit
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of items the queue can hold, or `None` for unlimited
    ///
    /// # Examples
    ///
    /// ```
    /// use des_components::queue::FifoQueue;
    ///
    /// let bounded = FifoQueue::new(Some(100));
    /// let unbounded = FifoQueue::new(None);
    /// ```
    pub fn new(capacity: Option<usize>) -> Self {
        Self {
            items: VecDeque::new(),
            capacity,
            total_enqueued: 0,
            total_dequeued: 0,
        }
    }

    /// Create a new FIFO queue with unlimited capacity
    pub fn unbounded() -> Self {
        Self::new(None)
    }

    /// Create a new FIFO queue with specified capacity
    pub fn bounded(capacity: usize) -> Self {
        Self::new(Some(capacity))
    }

    /// Get the total number of items ever enqueued
    pub fn total_enqueued(&self) -> u64 {
        self.total_enqueued
    }

    /// Get the total number of items ever dequeued
    pub fn total_dequeued(&self) -> u64 {
        self.total_dequeued
    }

    /// Get the current queue depth as a percentage of capacity
    ///
    /// Returns `None` if the queue has unlimited capacity.
    pub fn utilization(&self) -> Option<f64> {
        self.capacity
            .map(|cap| self.len() as f64 / cap as f64)
    }
}

impl Queue for FifoQueue {
    fn enqueue(&mut self, item: QueueItem) -> Result<(), QueueError> {
        if self.is_full() {
            return Err(QueueError::Full {
                capacity: self.capacity.unwrap(),
            });
        }

        self.items.push_back(item);
        self.total_enqueued += 1;
        Ok(())
    }

    fn dequeue(&mut self) -> Option<QueueItem> {
        let item = self.items.pop_front();
        if item.is_some() {
            self.total_dequeued += 1;
        }
        item
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn capacity(&self) -> Option<usize> {
        self.capacity
    }

    fn peek(&self) -> Option<&QueueItem> {
        self.items.front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::{RequestAttemptId, RequestId};

    fn create_test_item(id: u64, time: u64) -> QueueItem {
        let attempt = RequestAttempt::new(
            RequestAttemptId(id),
            RequestId(id),
            1,
            SimTime::from_millis(time),
            vec![],
        );
        QueueItem::new(attempt, SimTime::from_millis(time))
    }

    #[test]
    fn test_fifo_queue_basic_operations() {
        let mut queue = FifoQueue::new(None);
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);

        let item1 = create_test_item(1, 100);
        let item2 = create_test_item(2, 200);

        queue.enqueue(item1).unwrap();
        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());

        queue.enqueue(item2).unwrap();
        assert_eq!(queue.len(), 2);

        // FIFO order: first in, first out
        let dequeued1 = queue.dequeue().unwrap();
        assert_eq!(dequeued1.attempt.id, RequestAttemptId(1));

        let dequeued2 = queue.dequeue().unwrap();
        assert_eq!(dequeued2.attempt.id, RequestAttemptId(2));

        assert!(queue.is_empty());
        assert_eq!(queue.dequeue(), None);
    }

    #[test]
    fn test_fifo_queue_capacity() {
        let mut queue = FifoQueue::new(Some(2));
        assert_eq!(queue.capacity(), Some(2));
        assert!(!queue.is_full());

        queue.enqueue(create_test_item(1, 100)).unwrap();
        assert!(!queue.is_full());

        queue.enqueue(create_test_item(2, 200)).unwrap();
        assert!(queue.is_full());

        // Should fail when full
        let result = queue.enqueue(create_test_item(3, 300));
        assert!(matches!(result, Err(QueueError::Full { capacity: 2 })));

        // After dequeue, should have space again
        queue.dequeue();
        assert!(!queue.is_full());
        queue.enqueue(create_test_item(3, 300)).unwrap();
    }

    #[test]
    fn test_fifo_queue_peek() {
        let mut queue = FifoQueue::new(None);
        assert_eq!(queue.peek(), None);

        let item1 = create_test_item(1, 100);
        queue.enqueue(item1).unwrap();

        let peeked = queue.peek().unwrap();
        assert_eq!(peeked.attempt.id, RequestAttemptId(1));
        assert_eq!(queue.len(), 1); // Peek doesn't remove

        queue.enqueue(create_test_item(2, 200)).unwrap();
        let peeked = queue.peek().unwrap();
        assert_eq!(peeked.attempt.id, RequestAttemptId(1)); // Still first item
    }

    #[test]
    fn test_fifo_queue_metrics() {
        let mut queue = FifoQueue::new(None);
        assert_eq!(queue.total_enqueued(), 0);
        assert_eq!(queue.total_dequeued(), 0);

        queue.enqueue(create_test_item(1, 100)).unwrap();
        queue.enqueue(create_test_item(2, 200)).unwrap();
        assert_eq!(queue.total_enqueued(), 2);
        assert_eq!(queue.total_dequeued(), 0);

        queue.dequeue();
        assert_eq!(queue.total_enqueued(), 2);
        assert_eq!(queue.total_dequeued(), 1);

        queue.dequeue();
        assert_eq!(queue.total_enqueued(), 2);
        assert_eq!(queue.total_dequeued(), 2);
    }

    #[test]
    fn test_fifo_queue_utilization() {
        let unbounded = FifoQueue::unbounded();
        assert_eq!(unbounded.utilization(), None);

        let mut bounded = FifoQueue::bounded(10);
        assert_eq!(bounded.utilization(), Some(0.0));

        bounded.enqueue(create_test_item(1, 100)).unwrap();
        assert_eq!(bounded.utilization(), Some(0.1));

        for i in 2..=10 {
            bounded.enqueue(create_test_item(i, i * 100)).unwrap();
        }
        assert_eq!(bounded.utilization(), Some(1.0));
    }

    #[test]
    fn test_queue_item_creation() {
        let attempt = RequestAttempt::new(
            RequestAttemptId(1),
            RequestId(1),
            1,
            SimTime::from_millis(100),
            vec![1, 2, 3],
        );

        let item = QueueItem::new(attempt.clone(), SimTime::from_millis(100));
        assert_eq!(item.priority, 0);
        assert_eq!(item.enqueued_at, SimTime::from_millis(100));

        let item_with_priority =
            QueueItem::with_priority(attempt, SimTime::from_millis(100), 5);
        assert_eq!(item_with_priority.priority, 5);
    }

    #[test]
    fn test_queue_item_queue_time() {
        let attempt = RequestAttempt::new(
            RequestAttemptId(1),
            RequestId(1),
            1,
            SimTime::from_millis(100),
            vec![],
        );
        let item = QueueItem::new(attempt, SimTime::from_millis(100));

        let queue_time = item.queue_time(SimTime::from_millis(250));
        assert_eq!(queue_time, std::time::Duration::from_millis(150));
    }
}

/// Wrapper for priority-based ordering in BinaryHeap
///
/// BinaryHeap is a max-heap, but we want min-heap behavior (lower priority values
/// should be dequeued first). This wrapper reverses the ordering.
#[derive(Debug, Clone)]
struct PrioritizedItem {
    item: QueueItem,
    /// Sequence number for deterministic ordering of equal priorities
    sequence: u64,
}

impl PartialEq for PrioritizedItem {
    fn eq(&self, other: &Self) -> bool {
        self.item.priority == other.item.priority && self.sequence == other.sequence
    }
}

impl Eq for PrioritizedItem {}

impl PartialOrd for PrioritizedItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrioritizedItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse ordering: lower priority values come first
        // If priorities are equal, use sequence number (FIFO for same priority)
        other
            .item
            .priority
            .cmp(&self.item.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

/// Priority queue implementation
///
/// PriorityQueue processes items based on their priority value, with lower priority
/// values being dequeued first. Items with equal priority are processed in FIFO order.
/// It supports optional capacity limits.
///
/// # Requirements
///
/// - 2.3: Provide Queue component with configurable capacity and queuing disciplines
///
/// # Examples
///
/// ```
/// use des_components::queue::{PriorityQueue, QueueItem, Queue};
/// use des_components::{RequestAttempt, RequestAttemptId, RequestId};
/// use des_core::SimTime;
///
/// let mut queue = PriorityQueue::new(Some(10));
/// let attempt = RequestAttempt::new(
///     RequestAttemptId(1),
///     RequestId(1),
///     1,
///     SimTime::zero(),
///     vec![],
/// );
/// let item = QueueItem::with_priority(attempt, SimTime::zero(), 5);
/// queue.enqueue(item).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct PriorityQueue {
    /// Internal storage using BinaryHeap for priority-based ordering
    items: BinaryHeap<PrioritizedItem>,
    /// Optional capacity limit
    capacity: Option<usize>,
    /// Sequence counter for deterministic ordering
    next_sequence: u64,
    /// Total number of items enqueued (for metrics)
    total_enqueued: u64,
    /// Total number of items dequeued (for metrics)
    total_dequeued: u64,
}

impl PriorityQueue {
    /// Create a new priority queue with optional capacity limit
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of items the queue can hold, or `None` for unlimited
    ///
    /// # Examples
    ///
    /// ```
    /// use des_components::queue::PriorityQueue;
    ///
    /// let bounded = PriorityQueue::new(Some(100));
    /// let unbounded = PriorityQueue::new(None);
    /// ```
    pub fn new(capacity: Option<usize>) -> Self {
        Self {
            items: BinaryHeap::new(),
            capacity,
            next_sequence: 0,
            total_enqueued: 0,
            total_dequeued: 0,
        }
    }

    /// Create a new priority queue with unlimited capacity
    pub fn unbounded() -> Self {
        Self::new(None)
    }

    /// Create a new priority queue with specified capacity
    pub fn bounded(capacity: usize) -> Self {
        Self::new(Some(capacity))
    }

    /// Get the total number of items ever enqueued
    pub fn total_enqueued(&self) -> u64 {
        self.total_enqueued
    }

    /// Get the total number of items ever dequeued
    pub fn total_dequeued(&self) -> u64 {
        self.total_dequeued
    }

    /// Get the current queue depth as a percentage of capacity
    ///
    /// Returns `None` if the queue has unlimited capacity.
    pub fn utilization(&self) -> Option<f64> {
        self.capacity
            .map(|cap| self.len() as f64 / cap as f64)
    }
}

impl Queue for PriorityQueue {
    fn enqueue(&mut self, item: QueueItem) -> Result<(), QueueError> {
        if self.is_full() {
            return Err(QueueError::Full {
                capacity: self.capacity.unwrap(),
            });
        }

        let prioritized = PrioritizedItem {
            item,
            sequence: self.next_sequence,
        };
        self.next_sequence += 1;
        self.items.push(prioritized);
        self.total_enqueued += 1;
        Ok(())
    }

    fn dequeue(&mut self) -> Option<QueueItem> {
        let item = self.items.pop().map(|p| p.item);
        if item.is_some() {
            self.total_dequeued += 1;
        }
        item
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn capacity(&self) -> Option<usize> {
        self.capacity
    }

    fn peek(&self) -> Option<&QueueItem> {
        self.items.peek().map(|p| &p.item)
    }
}

#[cfg(test)]
mod priority_queue_tests {
    use super::*;
    use des_core::{RequestAttemptId, RequestId};

    fn create_test_item_with_priority(id: u64, time: u64, priority: u32) -> QueueItem {
        let attempt = RequestAttempt::new(
            RequestAttemptId(id),
            RequestId(id),
            1,
            SimTime::from_millis(time),
            vec![],
        );
        QueueItem::with_priority(attempt, SimTime::from_millis(time), priority)
    }

    #[test]
    fn test_priority_queue_basic_operations() {
        let mut queue = PriorityQueue::new(None);
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);

        let item1 = create_test_item_with_priority(1, 100, 10);
        let item2 = create_test_item_with_priority(2, 200, 5);
        let item3 = create_test_item_with_priority(3, 300, 15);

        queue.enqueue(item1).unwrap();
        queue.enqueue(item2).unwrap();
        queue.enqueue(item3).unwrap();
        assert_eq!(queue.len(), 3);

        // Should dequeue in priority order: 5, 10, 15
        let dequeued1 = queue.dequeue().unwrap();
        assert_eq!(dequeued1.attempt.id, RequestAttemptId(2));
        assert_eq!(dequeued1.priority, 5);

        let dequeued2 = queue.dequeue().unwrap();
        assert_eq!(dequeued2.attempt.id, RequestAttemptId(1));
        assert_eq!(dequeued2.priority, 10);

        let dequeued3 = queue.dequeue().unwrap();
        assert_eq!(dequeued3.attempt.id, RequestAttemptId(3));
        assert_eq!(dequeued3.priority, 15);

        assert!(queue.is_empty());
    }

    #[test]
    fn test_priority_queue_equal_priorities() {
        let mut queue = PriorityQueue::new(None);

        // All items have same priority - should be FIFO
        let item1 = create_test_item_with_priority(1, 100, 5);
        let item2 = create_test_item_with_priority(2, 200, 5);
        let item3 = create_test_item_with_priority(3, 300, 5);

        queue.enqueue(item1).unwrap();
        queue.enqueue(item2).unwrap();
        queue.enqueue(item3).unwrap();

        // Should dequeue in FIFO order when priorities are equal
        let dequeued1 = queue.dequeue().unwrap();
        assert_eq!(dequeued1.attempt.id, RequestAttemptId(1));

        let dequeued2 = queue.dequeue().unwrap();
        assert_eq!(dequeued2.attempt.id, RequestAttemptId(2));

        let dequeued3 = queue.dequeue().unwrap();
        assert_eq!(dequeued3.attempt.id, RequestAttemptId(3));
    }

    #[test]
    fn test_priority_queue_capacity() {
        let mut queue = PriorityQueue::new(Some(2));
        assert_eq!(queue.capacity(), Some(2));
        assert!(!queue.is_full());

        queue
            .enqueue(create_test_item_with_priority(1, 100, 10))
            .unwrap();
        assert!(!queue.is_full());

        queue
            .enqueue(create_test_item_with_priority(2, 200, 5))
            .unwrap();
        assert!(queue.is_full());

        // Should fail when full
        let result = queue.enqueue(create_test_item_with_priority(3, 300, 1));
        assert!(matches!(result, Err(QueueError::Full { capacity: 2 })));

        // After dequeue, should have space again
        queue.dequeue();
        assert!(!queue.is_full());
        queue
            .enqueue(create_test_item_with_priority(3, 300, 1))
            .unwrap();
    }

    #[test]
    fn test_priority_queue_peek() {
        let mut queue = PriorityQueue::new(None);
        assert_eq!(queue.peek(), None);

        queue
            .enqueue(create_test_item_with_priority(1, 100, 10))
            .unwrap();
        queue
            .enqueue(create_test_item_with_priority(2, 200, 5))
            .unwrap();

        let peeked = queue.peek().unwrap();
        assert_eq!(peeked.attempt.id, RequestAttemptId(2)); // Priority 5 is highest
        assert_eq!(queue.len(), 2); // Peek doesn't remove
    }

    #[test]
    fn test_priority_queue_metrics() {
        let mut queue = PriorityQueue::new(None);
        assert_eq!(queue.total_enqueued(), 0);
        assert_eq!(queue.total_dequeued(), 0);

        queue
            .enqueue(create_test_item_with_priority(1, 100, 10))
            .unwrap();
        queue
            .enqueue(create_test_item_with_priority(2, 200, 5))
            .unwrap();
        assert_eq!(queue.total_enqueued(), 2);
        assert_eq!(queue.total_dequeued(), 0);

        queue.dequeue();
        assert_eq!(queue.total_enqueued(), 2);
        assert_eq!(queue.total_dequeued(), 1);

        queue.dequeue();
        assert_eq!(queue.total_enqueued(), 2);
        assert_eq!(queue.total_dequeued(), 2);
    }

    #[test]
    fn test_priority_queue_utilization() {
        let unbounded = PriorityQueue::unbounded();
        assert_eq!(unbounded.utilization(), None);

        let mut bounded = PriorityQueue::bounded(10);
        assert_eq!(bounded.utilization(), Some(0.0));

        bounded
            .enqueue(create_test_item_with_priority(1, 100, 5))
            .unwrap();
        assert_eq!(bounded.utilization(), Some(0.1));

        for i in 2..=10 {
            bounded
                .enqueue(create_test_item_with_priority(i, i * 100, 5))
                .unwrap();
        }
        assert_eq!(bounded.utilization(), Some(1.0));
    }

    #[test]
    fn test_priority_queue_mixed_priorities() {
        let mut queue = PriorityQueue::new(None);

        // Mix of priorities
        queue
            .enqueue(create_test_item_with_priority(1, 100, 50))
            .unwrap();
        queue
            .enqueue(create_test_item_with_priority(2, 200, 10))
            .unwrap();
        queue
            .enqueue(create_test_item_with_priority(3, 300, 30))
            .unwrap();
        queue
            .enqueue(create_test_item_with_priority(4, 400, 10))
            .unwrap();
        queue
            .enqueue(create_test_item_with_priority(5, 500, 20))
            .unwrap();

        // Should dequeue: 10, 10, 20, 30, 50
        // For equal priorities (10), FIFO order: 2, 4
        assert_eq!(queue.dequeue().unwrap().attempt.id, RequestAttemptId(2));
        assert_eq!(queue.dequeue().unwrap().attempt.id, RequestAttemptId(4));
        assert_eq!(queue.dequeue().unwrap().attempt.id, RequestAttemptId(5));
        assert_eq!(queue.dequeue().unwrap().attempt.id, RequestAttemptId(3));
        assert_eq!(queue.dequeue().unwrap().attempt.id, RequestAttemptId(1));
    }
}
