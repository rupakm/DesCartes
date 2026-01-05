//! Error types for visualization

use thiserror::Error;

/// Errors related to visualization operations
#[derive(Debug, Error)]
pub enum VizError {
    #[error("Plot backend error: {0}")]
    BackendError(String),

    #[error("Invalid plot configuration: {0}")]
    InvalidConfiguration(String),
    
    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Export failed: {0}")]
    ExportFailed(String),

    #[error("Rendering error: {0}")]
    RenderingError(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("Plotting error: {0}")]
    PlottingError(String),
}

// Implement From for plotters errors
impl<T> From<plotters::drawing::DrawingAreaErrorKind<T>> for VizError 
where
    T: std::error::Error + Send + Sync + 'static,
{
    fn from(err: plotters::drawing::DrawingAreaErrorKind<T>) -> Self {
        VizError::PlottingError(format!("Drawing area error: {}", err))
    }
}
