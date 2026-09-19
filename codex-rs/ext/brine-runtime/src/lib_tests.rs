use std::sync::Arc;

use codex_brine_runtime::AttachmentSnapshot;
use codex_brine_runtime::InMemoryRuntimeAuthority;
use codex_brine_runtime::RemoteDelta;
use codex_brine_runtime::RuntimeState;
use codex_brine_runtime::SessionAttachment;
use codex_brine_runtime::SessionId;
use codex_brine_runtime::WorkId;
use codex_brine_runtime::WorkRecord;
use codex_brine_runtime::WorkspaceId;
use codex_brine_runtime::WorkspaceRecord;
use codex_extension_api::ExtensionRegistryBuilder;

use super::LocalWorkspace;
use super::attached_runtime;
use super::install;

#[test]
fn install_registers_only_lifecycle_attachment() {
    let mut builder = ExtensionRegistryBuilder::<()>::new();
    install(
        &mut builder,
        Arc::new(InMemoryRuntimeAuthority::default()),
        |_| None,
    );
    let registry = builder.build();
    assert_eq!(registry.thread_lifecycle_contributors().len(), 1);
    assert!(registry.context_contributors().is_empty());
}

#[test]
fn attachment_retains_runtime_revision_and_pending_deltas() {
    let workspace_id = WorkspaceId::from("workspace-1");
    let work_id = WorkId::from("work-1");
    let session_id = SessionId::from("session-2");
    let snapshot = AttachmentSnapshot {
        attachment: SessionAttachment {
            session_id,
            workspace_id: workspace_id.clone(),
            work_id: work_id.clone(),
            attached_revision: 4,
        },
        state: RuntimeState {
            revision: 4,
            workspace: WorkspaceRecord {
                id: workspace_id.clone(),
                key: "repo".to_owned(),
                repository_identity: "repo".to_owned(),
                created_revision: 1,
            },
            work: WorkRecord {
                id: work_id.clone(),
                workspace_id,
                key: "root".to_owned(),
                objective: "persistent work".to_owned(),
                created_revision: 2,
                last_revision: 4,
            },
            workspace_material: None,
            pending_deltas: vec![
                RemoteDelta {
                    revision: 3,
                    work_id: work_id.clone(),
                    summary: "first offline delta".to_owned(),
                },
                RemoteDelta {
                    revision: 4,
                    work_id,
                    summary: "second offline delta".to_owned(),
                },
            ],
        },
    };
    let local_workspace = LocalWorkspace {
        root: "C:\\workspace".into(),
        repository_identity: "local-only".to_owned(),
    };

    let attached = attached_runtime(snapshot, local_workspace.clone());

    assert_eq!(attached.remote.attached_revision, 4);
    assert_eq!(attached.state.revision, 4);
    assert_eq!(attached.state.pending_deltas.len(), 2);
    assert_eq!(
        attached.state.pending_deltas[1].summary,
        "second offline delta"
    );
    assert_eq!(attached.local_workspace, local_workspace);
}
