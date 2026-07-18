//! Local setup and immutable lexicon-pack lifecycle operations.

mod artifact;
mod audit;
mod error;
mod install;
mod registry;
mod release;
mod setup;
mod source_build;

pub use artifact::ArtifactBuildSummary;
pub use audit::{RepositoryAuditSummary, audit_repository};
pub use error::XtaskError;
pub use install::{InstallStatus, PackInstaller, verify_tool};
pub use registry::{PackRecord, PackRegistry};
pub use release::{ReleasePackageSummary, package_release};
pub use setup::{SetupReport, run_setup};
pub use source_build::build_from_source;
