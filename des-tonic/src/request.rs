use tonic::Request;

/// Tonic-style conversion into a request.
///
/// This matches the ergonomics of generated tonic client stubs.
pub trait IntoRequest<T> {
    fn into_request(self) -> Request<T>;
}

impl<T> IntoRequest<T> for T {
    fn into_request(self) -> Request<T> {
        Request::new(self)
    }
}

impl<T> IntoRequest<T> for Request<T> {
    fn into_request(self) -> Request<T> {
        self
    }
}
