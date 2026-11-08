pub mod detector;
pub mod runner;
pub mod scope;
pub mod transform;

pub use detector::{DetectorError, DetectorResolver};
pub use runner::{make_incomplete_run, SensorRunner};
pub use scope::{
    discover_files, filter_files_by_globs, matches_globs, ScopeArgs, ScopeError,
};
pub use transform::{evaluate_jq, TransformError};
