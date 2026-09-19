//! Persistent state for a BrineCDX session attachment.
//!
//! This crate deliberately owns logical runtime state only. A workspace root,
//! shell, process, and filesystem remain local to the Codex host that attaches
//! to this authority; they are never reconstructed from this state.

mod authority;
mod model;
mod wire;

pub use authority::FileRuntimeAuthority;
pub use authority::InMemoryRuntimeAuthority;
pub use authority::RuntimeAuthority;
pub use authority::RuntimeAuthorityError;
pub use model::AttachRequest;
pub use model::AttachmentSnapshot;
pub use model::ReconcileRequest;
pub use model::RemoteDelta;
pub use model::RuntimeState;
pub use model::SessionAttachment;
pub use model::SessionId;
pub use model::WorkId;
pub use model::WorkRecord;
pub use model::WorkspaceId;
pub use model::WorkspaceMaterialObservation;
pub use model::WorkspaceMaterialState;
pub use model::WorkspaceRevisionRecord;
pub use model::WorkspaceRecord;
pub use wire::RuntimeAuthorityServer;
pub use wire::TcpRuntimeAuthority;
