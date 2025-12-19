//! Builder pattern infrastructure for type-safe component construction
//!
//! This module provides utilities and patterns for building simulation components
//! with compile-time validation of required fields.
//!
//! # Type-State Builder Pattern
//!
//! The builders use the type-state pattern to enforce that required fields are set
//! at compile time. This prevents runtime errors from missing configuration.
//!
//! # Example
//!
//! ```ignore
//! use des_components::builder::*;
//!
//! // This will compile - all required fields are set
//! let component = MyComponentBuilder::new()
//!     .name("my-component")
//!     .required_field(42)
//!     .build();
//!
//! // This will NOT compile - missing required_field
//! let component = MyComponentBuilder::new()
//!     .name("my-component")
//!     .build(); // Compile error!
//! ```
//!
//! # Requirements
//!
//! - 6.2: Provide builder patterns for constructing complex Model_Component instances
//! - 6.3: Produce compile-time errors with descriptive messages for invalid parameters

/// Marker trait for builder state tracking
///
/// This trait is used with the type-state pattern to track which fields
/// have been set in a builder. Each state implements this trait.
pub trait BuilderState {}

/// Marker type indicating a required field has not been set
#[derive(Debug, Clone, Copy)]
pub struct Unset;
impl BuilderState for Unset {}

/// Marker type indicating a required field has been set
#[derive(Debug, Clone, Copy)]
pub struct Set;
impl BuilderState for Set {}

/// Helper trait for optional builder fields
///
/// This trait allows builder methods to accept both `T` and `Option<T>`,
/// making the API more ergonomic.
pub trait IntoOption<T> {
    fn into_option(self) -> Option<T>;
}

impl<T> IntoOption<T> for T {
    fn into_option(self) -> Option<T> {
        Some(self)
    }
}

impl<T> IntoOption<T> for Option<T> {
    fn into_option(self) -> Option<T> {
        self
    }
}

/// Validation result for builder configuration
pub type ValidationResult<T> = Result<T, ValidationError>;

/// Errors that can occur during builder validation
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Invalid value for field '{field}': {reason}")]
    InvalidValue { field: String, reason: String },

    #[error("Field '{field}' must be {constraint}")]
    ConstraintViolation { field: String, constraint: String },

    #[error("Configuration error: {0}")]
    Configuration(String),
}

/// Trait for validating builder configurations
///
/// Implement this trait on your builder to add custom validation logic
/// that runs when `build()` is called.
pub trait Validate {
    /// Validate the builder configuration
    ///
    /// This method is called automatically by `build()` and should check
    /// that all field values are valid and consistent with each other.
    ///
    /// # Errors
    ///
    /// Returns a `ValidationError` if validation fails.
    fn validate(&self) -> ValidationResult<()> {
        Ok(())
    }
}

/// Helper macro for creating type-state builders
///
/// This macro generates the boilerplate for a type-state builder pattern,
/// including state transitions and compile-time validation.
///
/// # Example
///
/// ```ignore
/// builder_struct! {
///     /// Documentation for MyComponent
///     pub struct MyComponentBuilder {
///         // Required fields use type parameters
///         name: String => NameState,
///         capacity: usize => CapacityState,
///         
///         // Optional fields use Option<T>
///         timeout: Option<Duration>,
///         max_retries: Option<usize>,
///     }
/// }
/// ```
#[macro_export]
macro_rules! builder_struct {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field:ident : $field_ty:ty $(=> $state:ident)?
            ),* $(,)?
        }
    ) => {
        // Generate the builder struct with type parameters for required fields
        $(#[$meta])*
        $vis struct $name<$($($state = $crate::builder::Unset,)?)*> {
            $(
                $(#[$field_meta])*
                $field: $field_ty,
            )*
            $(
                $(
                    _phantom_$state: std::marker::PhantomData<$state>,
                )?
            )*
        }
    };
}

/// Helper macro for generating builder setter methods
///
/// This macro generates setter methods that transition between type states
/// for required fields, ensuring compile-time validation.
#[macro_export]
macro_rules! builder_setter {
    // Required field setter (transitions from Unset to Set)
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($field:ident: $ty:ty) -> $state:ident
    ) => {
        $(#[$meta])*
        $vis fn $name(self, $field: $ty) -> Self::$state {
            Self::$state {
                $field,
                ..self
            }
        }
    };
    
    // Optional field setter (no state transition)
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($field:ident: $ty:ty)
    ) => {
        $(#[$meta])*
        $vis fn $name(mut self, $field: impl $crate::builder::IntoOption<$ty>) -> Self {
            self.$field = $field.into_option();
            self
        }
    };
}

/// Helper for validating numeric ranges
pub fn validate_range<T: PartialOrd + std::fmt::Display>(
    field: &str,
    value: T,
    min: T,
    max: T,
) -> ValidationResult<()> {
    if value < min || value > max {
        Err(ValidationError::ConstraintViolation {
            field: field.to_string(),
            constraint: format!("between {min} and {max}"),
        })
    } else {
        Ok(())
    }
}

/// Helper for validating that a value is positive
pub fn validate_positive<T: PartialOrd + Default + std::fmt::Display>(
    field: &str,
    value: T,
) -> ValidationResult<()> {
    if value <= T::default() {
        Err(ValidationError::ConstraintViolation {
            field: field.to_string(),
            constraint: "positive".to_string(),
        })
    } else {
        Ok(())
    }
}

/// Helper for validating that a value is non-negative
pub fn validate_non_negative<T: PartialOrd + Default + std::fmt::Display>(
    field: &str,
    value: T,
) -> ValidationResult<()> {
    if value < T::default() {
        Err(ValidationError::ConstraintViolation {
            field: field.to_string(),
            constraint: "non-negative".to_string(),
        })
    } else {
        Ok(())
    }
}

/// Helper for validating that a string is not empty
pub fn validate_non_empty(field: &str, value: &str) -> ValidationResult<()> {
    if value.is_empty() {
        Err(ValidationError::InvalidValue {
            field: field.to_string(),
            reason: "cannot be empty".to_string(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_range() {
        assert!(validate_range("test", 5, 0, 10).is_ok());
        assert!(validate_range("test", 0, 0, 10).is_ok());
        assert!(validate_range("test", 10, 0, 10).is_ok());
        assert!(validate_range("test", -1, 0, 10).is_err());
        assert!(validate_range("test", 11, 0, 10).is_err());
    }

    #[test]
    fn test_validate_positive() {
        assert!(validate_positive("test", 1).is_ok());
        assert!(validate_positive("test", 0).is_err());
        assert!(validate_positive("test", -1).is_err());
    }

    #[test]
    fn test_validate_non_negative() {
        assert!(validate_non_negative("test", 1).is_ok());
        assert!(validate_non_negative("test", 0).is_ok());
        assert!(validate_non_negative("test", -1).is_err());
    }

    #[test]
    fn test_validate_non_empty() {
        assert!(validate_non_empty("test", "hello").is_ok());
        assert!(validate_non_empty("test", "").is_err());
    }
}
