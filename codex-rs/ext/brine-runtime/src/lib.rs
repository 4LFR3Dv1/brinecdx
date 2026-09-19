//! Codex lifecycle attachment for the persistent Brine runtime.
//!
//! The extension records only the logical attachment in the remote authority.
//! The local workspace root stays in the host thread store and is never sent to
//! the authority or exposed as model-visible context.

mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use codex_brine_runtime::AttachRequest;
use codex_brine_runtime::AttachmentSnapshot;
use codex_brine_runtime::ReconcileRequest;
use codex_brine_runtime::RuntimeAuthority;
use codex_brine_runtime::RuntimeAuthorityError;
use codex_brine_runtime::RuntimeState;
use codex_brine_runtime::SessionAttachment;
use codex_brine_runtime::SessionId;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadResumeInput;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ThreadStopInput;

pub use workspace::observe_local_workspace;

/// Local physical reality owned by the Codex host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalWorkspace {
    /// Filesystem root used by local shell and tools.
    pub root: PathBuf,
    /// Host-observed repository identity, if known.
    pub repository_identity: String,
}

/// Inputs required to attach one Codex thread to persistent work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAttachmentConfig {
    /// Stable logical workspace key understood by the remote authority.
    pub workspace_key: String,
    /// Repository identity observed by the local host.
    pub repository_identity: String,
    /// Stable logical work key. Empty selects the workspace's persistent root Work.
    pub work_key: String,
    /// Objective associated with a newly-created work record.
    pub objective: String,
    /// Local filesystem reality; this is never persisted by the authority.
    pub local_workspace: LocalWorkspace,
}

/// Runtime attachment retained in the host-owned thread store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachedRuntime {
    /// Durable remote identity and attachment revision returned by the authority.
    pub remote: SessionAttachment,
    /// Latest durable runtime state observed by this Codex thread.
    ///
    /// This remains host-visible only in R1: it is not contributed to model context.
    pub state: RuntimeState,
    /// Local physical reality used by shell and filesystem tools.
    pub local_workspace: LocalWorkspace,
}

type ConfigResolver<C> = dyn Fn(&C) -> Option<SessionAttachmentConfig> + Send + Sync;

/// A lifecycle contributor that attaches and reconciles persistent Brine work.
pub struct BrineRuntimeExtension<C> {
    authority: Arc<dyn RuntimeAuthority>,
    config: Arc<ConfigResolver<C>>,
}

impl<C> std::fmt::Debug for BrineRuntimeExtension<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrineRuntimeExtension")
            .finish_non_exhaustive()
    }
}

impl<C> BrineRuntimeExtension<C> {
    /// Creates an extension using a host-owned remote runtime authority.
    pub fn new(
        authority: Arc<dyn RuntimeAuthority>,
        config: impl Fn(&C) -> Option<SessionAttachmentConfig> + Send + Sync + 'static,
    ) -> Self {
        Self {
            authority,
            config: Arc::new(config),
        }
    }
}

/// Installs the Brine lifecycle contributor without changing model input.
pub fn install<C: Sync + 'static>(
    builder: &mut ExtensionRegistryBuilder<C>,
    authority: Arc<dyn RuntimeAuthority>,
    config: impl Fn(&C) -> Option<SessionAttachmentConfig> + Send + Sync + 'static,
) {
    builder.thread_lifecycle_contributor(Arc::new(BrineRuntimeExtension::new(authority, config)));
}

impl<C: Sync> ThreadLifecycleContributor<C> for BrineRuntimeExtension<C> {
    fn on_thread_start<'a>(&'a self, input: ThreadStartInput<'a, C>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let Some(config) = (self.config)(input.config) else {
                return;
            };
            let session_id = SessionId::from(input.session_store.level_id());
            let material = observe_material_or_warn(&config.local_workspace);
            let result = self.authority.attach(AttachRequest {
                session_id,
                workspace_key: config.workspace_key,
                repository_identity: config.repository_identity.clone(),
                work_key: config.work_key,
                objective: config.objective,
                since_revision: None,
                material,
            });
            match result {
                Ok(snapshot) => {
                    let attached = attached_runtime(snapshot, config.local_workspace);
                    log_attachment_success("attach", &attached);
                    input.thread_store.insert(attached);
                }
                Err(error) => log_attachment_error("attach", error),
            }
        })
    }

    fn on_thread_resume<'a>(&'a self, input: ThreadResumeInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let Some(current) = input.thread_store.get::<AttachedRuntime>() else {
                return;
            };
            let material = observe_material_or_warn(&current.local_workspace);
            let result = self.authority.reconcile(ReconcileRequest {
                session_id: current.remote.session_id.clone(),
                workspace_id: current.remote.workspace_id.clone(),
                work_id: current.remote.work_id.clone(),
                since_revision: Some(current.remote.attached_revision),
                material,
            });
            match result {
                Ok(snapshot) => {
                    let attached = attached_runtime(snapshot, current.local_workspace.clone());
                    log_attachment_success("reconcile", &attached);
                    input.thread_store.insert(attached);
                }
                Err(error) => log_attachment_error("reconcile", error),
            }
        })
    }

    fn on_thread_stop<'a>(&'a self, input: ThreadStopInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let Some(current) = input.thread_store.get::<AttachedRuntime>() else {
                return;
            };
            if let Err(error) = self.authority.detach(&current.remote.session_id) {
                log_attachment_error("detach", error);
            }
        })
    }
}

fn observe_material_or_warn(
    workspace: &LocalWorkspace,
) -> Option<codex_brine_runtime::WorkspaceMaterialObservation> {
    match observe_local_workspace(workspace) {
        Ok(material) => material,
        Err(error) => {
            tracing::warn!(
                %error,
                root = %workspace.root.display(),
                "Brine workspace material observation failed"
            );
            None
        }
    }
}

fn attached_runtime(
    snapshot: AttachmentSnapshot,
    local_workspace: LocalWorkspace,
) -> AttachedRuntime {
    AttachedRuntime {
        remote: snapshot.attachment,
        state: snapshot.state,
        local_workspace,
    }
}

fn log_attachment_success(operation: &str, attached: &AttachedRuntime) {
    tracing::info!(
        operation,
        workspace_id = %attached.remote.workspace_id,
        work_id = %attached.remote.work_id,
        session_id = %attached.remote.session_id,
        attached_revision = attached.remote.attached_revision,
        runtime_revision = attached.state.revision,
        pending_delta_count = attached.state.pending_deltas.len(),
        "Brine runtime session attachment succeeded"
    );
}

fn log_attachment_error(operation: &str, error: RuntimeAuthorityError) {
    tracing::warn!(operation, %error, "Brine runtime session attachment failed");
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
