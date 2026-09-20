use std::sync::Arc;

use codex_brine_runtime::AttachmentSnapshot;
use codex_brine_runtime::InMemoryRuntimeAuthority;
use codex_brine_runtime::RemoteDelta;
use codex_brine_runtime::RUNTIME_PROTOCOL_VERSION;
use codex_brine_runtime::RuntimeState;
use codex_brine_runtime::SessionAttachment;
use codex_brine_runtime::SessionId;
use codex_brine_runtime::WorkId;
use codex_brine_runtime::WorkRecord;
use codex_brine_runtime::WorkStatus;
use codex_brine_runtime::WorkspaceId;
use codex_brine_runtime::WorkspaceRecord;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolCallOutcome;

use super::LocalWorkspace;
use super::attached_runtime;
use super::install;
use super::outcome_may_have_mutated;
use super::snapshot_protocol_compatible;

#[test]
fn install_registers_runtime_observers_without_model_context() {
    let mut builder = ExtensionRegistryBuilder::<()>::new();
    install(
        &mut builder,
        Arc::new(InMemoryRuntimeAuthority::default()),
        |_| None,
    );
    let registry = builder.build();
    assert_eq!(registry.thread_lifecycle_contributors().len(), 1);
    assert_eq!(registry.turn_lifecycle_contributors().len(), 1);
    assert_eq!(registry.tool_lifecycle_contributors().len(), 1);
    assert!(registry.context_contributors().is_empty());
}

#[test]
fn attachment_retains_runtime_revision_and_pending_deltas() {
    let workspace_id = WorkspaceId::from("workspace-1");
    let work_id = WorkId::from("work-1");
    let session_id = SessionId::from("session-2");
    let snapshot = AttachmentSnapshot {
        protocol_version: RUNTIME_PROTOCOL_VERSION,
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
                parent_work: None,
                objective: "persistent work".to_owned(),
                status: WorkStatus::Active,
                assigned_thread: None,
                candidate: None,
                created_revision: 2,
                last_revision: 4,
            },
            work_graph: None,
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


#[test]
fn incompatible_runtime_protocol_is_rejected() {
    let workspace_id = WorkspaceId::from("workspace-protocol");
    let work_id = WorkId::from("work-protocol");
    let snapshot = AttachmentSnapshot {
        protocol_version: RUNTIME_PROTOCOL_VERSION + 1,
        attachment: SessionAttachment {
            session_id: SessionId::from("session-protocol"),
            workspace_id: workspace_id.clone(),
            work_id: work_id.clone(),
            attached_revision: 1,
        },
        state: RuntimeState {
            revision: 1,
            workspace: WorkspaceRecord {
                id: workspace_id.clone(),
                key: "repo".to_owned(),
                repository_identity: "repo".to_owned(),
                created_revision: 1,
            },
            work: WorkRecord {
                id: work_id,
                workspace_id,
                key: "root".to_owned(),
                parent_work: None,
                objective: "protocol test".to_owned(),
                status: WorkStatus::Active,
                assigned_thread: None,
                candidate: None,
                created_revision: 1,
                last_revision: 1,
            },
            work_graph: None,
            workspace_material: None,
            pending_deltas: Vec::new(),
        },
    };

    assert!(!snapshot_protocol_compatible("test", &snapshot));
}

#[test]
fn mutation_observation_runs_only_when_execution_could_have_happened() {
    assert!(outcome_may_have_mutated(ToolCallOutcome::Completed {
        success: true,
    }));
    assert!(outcome_may_have_mutated(ToolCallOutcome::Completed {
        success: false,
    }));
    assert!(outcome_may_have_mutated(ToolCallOutcome::Failed {
        handler_executed: true,
    }));
    assert!(outcome_may_have_mutated(ToolCallOutcome::Aborted));
    assert!(!outcome_may_have_mutated(ToolCallOutcome::Blocked));
    assert!(!outcome_may_have_mutated(ToolCallOutcome::Failed {
        handler_executed: false,
    }));
}
