//! Reusable simulation components for distributed systems modeling
//!
//! This crate provides composable building blocks for simulating distributed systems,
//! including servers, clients, queues, throttles, and retry policies.

pub mod builder;
// Note: client.rs was removed due to incompatibility with current Component API
// Use simple_client.rs for basic client functionality
pub mod error;
pub mod retry_policy;

pub mod server;
pub mod simple_client;
pub mod tonic;
pub mod tower;
pub mod transport;

// Export distribution patterns from des-core
pub use des_core::dists::{
    AndPredicate, ArrivalPattern, BodySizePredicate, ClientInfo, CompositeServiceTime,
    ConstantArrivalPattern, ConstantServiceTime, EndpointBasedServiceTime, ExponentialDistribution,
    HeaderPredicate, MethodPredicate, OrPredicate, RequestContext, RequestPredicate,
    RequestSizeBasedServiceTime, ServiceTimeDistribution, UniformDistribution, UriExactPredicate,
    UriPrefixPredicate,
};

pub use retry_policy::{
    ExponentialBackoffPolicy, FixedRetryPolicy, NoRetryPolicy, RetryPolicy,
    SuccessBasedRetryPolicy, TokenBucketRetryPolicy,
};
pub use server::{Server, ServerEvent};
pub use simple_client::{ClientEvent, SimpleClient};
pub use tower::retry::{
    exponential_backoff_layer, DesRetryLayer, DesRetryPolicy, ExponentialBackoff,
};
pub use tower::{
    DesCircuitBreaker, DesConcurrencyLimit, DesGlobalConcurrencyLimit, DesHedge,
    DesLoadBalanceStrategy, DesLoadBalancer, DesRateLimit, DesTimeout,
};
pub use tower::{DesService, DesServiceBuilder, SchedulerHandle, ServiceError, SimBody};
pub use transport::{
    EndpointId, EndpointInfo, EndpointRegistry, LatencyConfig, LatencyJitterModel, MessageType,
    NetworkModel, SharedEndpointRegistry, SimEndpointRegistry, SimTransport, SimpleNetworkModel,
    TransportEvent, TransportMessage,
};
pub use tonic::{
    codec::{JsonCodec, ProtobufCodec}, MethodDescriptor, RpcCodec, RpcEvent, RpcRequest, RpcResponse,
    RpcService, RpcStatus, RpcStatusCode, SimTonicClient, SimTonicServer, TonicClientBuilder,
    TonicClientComponent, TonicError, TonicResult, TonicServerBuilder, TonicServerComponent,
};

pub use builder::{
    validate_non_empty, validate_non_negative, validate_positive, validate_range, BuilderState,
    IntoOption, Set, Unset, Validate, ValidationError, ValidationResult,
};
pub use error::{ComponentError, QueueError, RequestError, ThrottleError};

pub mod queue;
pub use queue::{FifoQueue, PriorityQueue, Queue, QueueItem};

pub use des_core::{
    AttemptStatus, Request, RequestAttempt, RequestAttemptId, RequestId, RequestStatus, Response,
    ResponseStatus,
};
