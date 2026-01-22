# Queue Systems

Queue components provide request buffering and ordering for servers when capacity is exceeded. DES-Components includes FIFO and priority-based queue implementations with configurable capacity limits and comprehensive metrics.

## Queue Trait

All queues implement the `Queue` trait, providing a consistent interface:

```rust
use des_components::queue::{Queue, QueueItem, QueueError};

pub trait Queue: Send {
    fn enqueue(&mut self, item: QueueItem) -> Result<(), QueueError>;
    fn dequeue(&mut self) -> Option<QueueItem>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn capacity(&self) -> Option<usize>;
    fn is_full(&self) -> bool;
    fn peek(&self) -> Option<&QueueItem>;
}
```

## Queue Items

Queue items wrap request attempts with metadata for queue management:

```rust
use des_components::queue::QueueItem;
use des_core::{RequestAttempt, RequestAttemptId, RequestId, SimTime};

// Basic queue item
let attempt = RequestAttempt::new(
    RequestAttemptId(1),
    RequestId(1),
    1,
    SimTime::zero(),
    b"request data".to_vec(),
);

let item = QueueItem::new(attempt, SimTime::from_millis(100));

// Queue item with priority (for priority queues)
let priority_item = QueueItem::with_priority(
    attempt,
    SimTime::from_millis(100),
    5, // priority level (lower = higher priority)
);

// Queue item with client ID for response routing
let client_item = QueueItem::with_client_id(
    attempt,
    SimTime::from_millis(100),
    client_uuid,
);
```

## FIFO Queue

First-In-First-Out queues process items in arrival order:

### Basic FIFO Queue

```rust
use des_components::queue::{FifoQueue, Queue};

// Unbounded FIFO queue
let mut unbounded_queue = FifoQueue::unbounded();

// Bounded FIFO queue
let mut bounded_queue = FifoQueue::bounded(100); // capacity of 100

// Queue with optional capacity
let mut optional_queue = FifoQueue::new(Some(50));
```

### FIFO Queue Operations

```rust
use des_components::queue::{FifoQueue, QueueItem, Queue};
use des_core::{RequestAttempt, RequestAttemptId, RequestId, SimTime};

let mut queue = FifoQueue::bounded(10);

// Create and enqueue items
for i in 1..=5 {
    let attempt = RequestAttempt::new(
        RequestAttemptId(i),
        RequestId(i),
        1,
        SimTime::from_millis(i * 100),
        format!("request {}", i).into_bytes(),
    );
    
    let item = QueueItem::new(attempt, SimTime::from_millis(i * 100));
    
    match queue.enqueue(item) {
        Ok(()) => println!("Enqueued request {}", i),
        Err(e) => println!("Failed to enqueue: {:?}", e),
    }
}

// Dequeue items (FIFO order)
while let Some(item) = queue.dequeue() {
    println!("Dequeued request {}", item.attempt.request_id.0);
    
    // Calculate time spent in queue
    let queue_time = item.queue_time(SimTime::from_millis(1000));
    println!("  Queue time: {:?}", queue_time);
}
```

### FIFO Queue Metrics

```rust
let queue = FifoQueue::bounded(50);

// Basic metrics
println!("Current length: {}", queue.len());
println!("Capacity: {:?}", queue.capacity());
println!("Is full: {}", queue.is_full());
println!("Is empty: {}", queue.is_empty());

// Throughput metrics
println!("Total enqueued: {}", queue.total_enqueued());
println!("Total dequeued: {}", queue.total_dequeued());

// Utilization (for bounded queues)
if let Some(utilization) = queue.utilization() {
    println!("Queue utilization: {:.1}%", utilization * 100.0);
}
```

## Priority Queue

Priority queues process items based on priority values, with lower values having higher priority:

### Basic Priority Queue

```rust
use des_components::queue::{PriorityQueue, Queue};

// Unbounded priority queue
let mut unbounded_pq = PriorityQueue::unbounded();

// Bounded priority queue
let mut bounded_pq = PriorityQueue::bounded(100);

// Queue with optional capacity
let mut optional_pq = PriorityQueue::new(Some(50));
```

### Priority Queue Operations

```rust
use des_components::queue::{PriorityQueue, QueueItem, Queue};
use des_core::{RequestAttempt, RequestAttemptId, RequestId, SimTime};

let mut pq = PriorityQueue::bounded(20);

// Enqueue items with different priorities
let requests = [
    (1, 10), // Request 1, priority 10 (low priority)
    (2, 5),  // Request 2, priority 5 (medium priority)
    (3, 1),  // Request 3, priority 1 (high priority)
    (4, 5),  // Request 4, priority 5 (medium priority)
];

for (id, priority) in requests {
    let attempt = RequestAttempt::new(
        RequestAttemptId(id),
        RequestId(id),
        1,
        SimTime::from_millis(id * 100),
        format!("request {}", id).into_bytes(),
    );
    
    let item = QueueItem::with_priority(
        attempt,
        SimTime::from_millis(id * 100),
        priority,
    );
    
    pq.enqueue(item).unwrap();
}

// Dequeue items (priority order: 3, 2, 4, 1)
// Items with equal priority are dequeued in FIFO order
while let Some(item) = pq.dequeue() {
    println!("Dequeued request {} (priority {})", 
        item.attempt.request_id.0, 
        item.priority);
}
```

## Queue Integration with Servers

Queues are typically used with servers to buffer requests during high load:

### Server with FIFO Queue

```rust
use des_components::{Server, FifoQueue};
use std::time::Duration;

let server = Server::with_constant_service_time(
    "buffered-server".to_string(),
    4, // 4 concurrent threads
    Duration::from_millis(100),
).with_queue(Box::new(FifoQueue::bounded(25))); // 25 request buffer
```

### Server with Priority Queue

```rust
use des_components::{Server, PriorityQueue};
use std::time::Duration;

let server = Server::with_exponential_service_time(
    "priority-server".to_string(),
    6,
    Duration::from_millis(80),
).with_queue(Box::new(PriorityQueue::bounded(40))); // Priority-based buffering
```

## Complete Queue Example

Here's a comprehensive example demonstrating different queue behaviors:

```rust
use des_components::{
    Server, ServerEvent, SimpleClient, ClientEvent,
    FifoQueue, PriorityQueue, NoRetryPolicy
};
use des_core::{
    Simulation, Execute, Executor, SimTime, 
    RequestAttempt, RequestAttemptId, RequestId,
    task::PeriodicTask
};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Queue Behavior Comparison ===\n");
    
    // Test FIFO queue behavior
    test_fifo_queue();
    
    // Test Priority queue behavior  
    test_priority_queue();
    
    // Test queue integration with servers
    test_server_integration()?;
    
    Ok(())
}

fn test_fifo_queue() {
    println!("1. FIFO Queue Test");
    
    let mut queue = FifoQueue::bounded(5);
    
    // Fill queue
    for i in 1..=7 {
        let attempt = RequestAttempt::new(
            RequestAttemptId(i),
            RequestId(i),
            1,
            SimTime::from_millis(i * 10),
            vec![],
        );
        
        let item = des_components::queue::QueueItem::new(
            attempt, 
            SimTime::from_millis(i * 10)
        );
        
        match queue.enqueue(item) {
            Ok(()) => println!("   Enqueued request {}", i),
            Err(e) => println!("   Failed to enqueue request {}: {:?}", i, e),
        }
    }
    
    println!("   Queue length: {}/{:?}", queue.len(), queue.capacity());
    
    // Drain queue
    while let Some(item) = queue.dequeue() {
        println!("   Dequeued request {} (FIFO order)", item.attempt.request_id.0);
    }
    
    println!("   Final metrics: {} enqueued, {} dequeued\n", 
        queue.total_enqueued(), queue.total_dequeued());
}

fn test_priority_queue() {
    println!("2. Priority Queue Test");
    
    let mut pq = PriorityQueue::bounded(10);
    
    // Add items with mixed priorities
    let items = [(1, 5), (2, 1), (3, 10), (4, 1), (5, 5)];
    
    for (id, priority) in items {
        let attempt = RequestAttempt::new(
            RequestAttemptId(id),
            RequestId(id),
            1,
            SimTime::from_millis(id * 10),
            vec![],
        );
        
        let item = des_components::queue::QueueItem::with_priority(
            attempt,
            SimTime::from_millis(id * 10),
            priority,
        );
        
        pq.enqueue(item).unwrap();
        println!("   Enqueued request {} with priority {}", id, priority);
    }
    
    println!("   Queue length: {}", pq.len());
    
    // Drain queue (should be priority order: 2, 4, 1, 5, 3)
    while let Some(item) = pq.dequeue() {
        println!("   Dequeued request {} (priority {})", 
            item.attempt.request_id.0, item.priority);
    }
    
    println!();
}

fn test_server_integration() -> Result<(), Box<dyn std::error::Error>> {
    println!("3. Server Integration Test");
    
    let mut sim = Simulation::default();
    
    // Server with small capacity and FIFO queue
    let server = Server::with_constant_service_time(
        "queue-server".to_string(),
        1, // Only 1 thread to force queuing
        Duration::from_millis(200), // Slow service
    ).with_queue(Box::new(FifoQueue::bounded(3))); // Small queue
    
    let server_id = sim.add_component(server);
    
    // Client that sends requests faster than server can process
    let client = SimpleClient::new(
        "fast-client".to_string(),
        server_id,
        Duration::from_millis(50), // Fast request rate
        NoRetryPolicy::new(), // No retries to see queue behavior
    ).with_max_requests(8); // Send 8 requests total
    
    let client_id = sim.add_component(client);
    
    // Start client
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(50)),
        8,
    );
    sim.schedule_task(SimTime::zero(), task);
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(5)))
        .execute(&mut sim);
    
    // Analyze results
    let final_server = sim.remove_component::<ServerEvent, Server>(server_id)?;
    let final_client = sim.remove_component::<ClientEvent, SimpleClient<NoRetryPolicy>>(client_id)?;
    
    println!("   Server Results:");
    println!("     Requests processed: {}", final_server.requests_processed);
    println!("     Requests rejected: {}", final_server.requests_rejected);
    println!("     Final queue depth: {}", final_server.queue_depth());
    println!("     Final utilization: {:.1}%", final_server.utilization() * 100.0);
    
    println!("   Client Results:");
    println!("     Requests sent: {}", final_client.requests_sent);
    
    // Demonstrate queue overflow behavior
    let total_requests = final_client.requests_sent;
    let processed = final_server.requests_processed;
    let rejected = final_server.requests_rejected;
    let queued = final_server.queue_depth();
    
    println!("   Queue Analysis:");
    println!("     Total: {} = Processed: {} + Rejected: {} + Queued: {}", 
        total_requests, processed, rejected, queued);
    
    if rejected > 0 {
        println!("     ⚠️  Queue overflow occurred - {} requests rejected", rejected);
    }
    
    Ok(())
}
```

## Queue Configuration Patterns

### High-Throughput Queuing

For high-throughput scenarios with minimal latency:

```rust
// Small queue for fast rejection when overloaded
let ht_queue = FifoQueue::bounded(server_capacity * 2);
```

### Batch Processing Queuing

For batch processing with variable load:

```rust
// Large queue to smooth out load spikes
let batch_queue = FifoQueue::bounded(server_capacity * 20);
```

### Priority-Based Service

For systems with different service classes:

```rust
// Priority queue with capacity for mixed workloads
let priority_queue = PriorityQueue::bounded(100);

// Assign priorities based on request type:
// - Priority 1: Critical/real-time requests
// - Priority 5: Normal requests  
// - Priority 10: Background/batch requests
```

### Unlimited Buffering

For systems that must never drop requests:

```rust
// Unbounded queue (use with caution - can cause memory issues)
let unlimited_queue = FifoQueue::unbounded();
```

## Best Practices

### Queue Sizing

Choose queue sizes based on your requirements:
- **Small queues** (1-5x server capacity): Fast failure, low latency
- **Medium queues** (5-20x server capacity): Balance between buffering and latency
- **Large queues** (20x+ server capacity): High buffering, potential high latency

### Priority Assignment

Design priority schemes carefully:
- Use few priority levels (3-5) to avoid complexity
- Reserve highest priority for truly critical requests
- Document priority meanings for your system

### Monitoring

Monitor queue metrics to detect issues:
- High queue depth indicates server overload
- High rejection rates indicate insufficient capacity
- Queue utilization trends show load patterns

### Error Handling

Handle queue-related errors appropriately:

```rust
match queue.enqueue(item) {
    Ok(()) => {
        // Item queued successfully
        metrics.increment_counter("requests_queued");
    }
    Err(QueueError::Full { capacity }) => {
        // Queue is full - reject request
        metrics.increment_counter("requests_rejected_queue_full");
        send_rejection_response(503, "Service Unavailable - Queue Full");
    }
}
```

## Performance Considerations

### Memory Usage

- FIFO queues use `VecDeque` for efficient front/back operations
- Priority queues use `BinaryHeap` for O(log n) priority operations
- Bounded queues prevent unbounded memory growth

### Time Complexity

- **FIFO enqueue/dequeue**: O(1) amortized
- **Priority enqueue**: O(log n)
- **Priority dequeue**: O(log n)
- **Peek operations**: O(1)

### Thread Safety

Queues are designed for single-threaded discrete event simulation but can be wrapped in `Arc<Mutex<>>` for multi-threaded scenarios if needed.

Queue systems provide essential buffering and ordering capabilities for realistic server modeling, enabling simulation of various load conditions and service disciplines in distributed systems.