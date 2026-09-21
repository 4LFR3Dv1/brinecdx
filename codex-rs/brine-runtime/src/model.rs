use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::Serialize;

pub const RUNTIME_PROTOCOL_VERSION: u32 = 3;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

id_type!(WorkspaceId);
id_type!(WorkId);
id_type!(SessionId);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub key: String,
    pub repository_identity: String,
    pub created_revision: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    #[default]
    Active,
    Paused,
    Complete,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkRecord {
    pub id: WorkId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    #[serde(default)]
    pub parent_work: Option<WorkId>,
    pub objective: String,
    #[serde(default)]
    pub status: WorkStatus,
    #[serde(default)]
    pub assigned_thread: Option<SessionId>,
    #[serde(default)]
    pub candidate: Option<String>,
    pub created_revision: u64,
    pub last_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkGraphSnapshot {
    pub active_work: WorkId,
    pub nodes: Vec<WorkRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoteDelta {
    pub revision: u64,
    pub work_id: WorkId,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceStructureObservation {
    pub digest: String,
    pub symbols: Vec<StructuralSymbol>,
    pub unresolved_relations: Vec<UnresolvedStructuralRelation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StructuralSymbol {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UnresolvedStructuralRelation {
    pub path: String,
    pub kind: String,
    pub target: String,
    pub line: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceMaterialObservation {
    pub head: Option<String>,
    pub index_state: String,
    pub working_tree: String,
    pub changed_paths: Vec<String>,
    pub file_digests: BTreeMap<String, String>,
    pub material_digest: String,
    #[serde(default)]
    pub structure: WorkspaceStructureObservation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceMaterialState {
    pub workspace_id: WorkspaceId,
    pub material_revision: u64,
    #[serde(default)]
    pub structural_revision: u64,
    pub runtime_revision: u64,
    pub observation: WorkspaceMaterialObservation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceRevisionRecord {
    pub workspace_id: WorkspaceId,
    pub material_revision: u64,
    #[serde(default)]
    pub structural_revision: u64,
    pub runtime_revision: u64,
    pub material_digest: String,
    #[serde(default)]
    pub structure_digest: String,
    pub head: Option<String>,
    pub changed_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionAttachment {
    pub session_id: SessionId,
    pub workspace_id: WorkspaceId,
    pub work_id: WorkId,
    pub attached_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeState {
    pub revision: u64,
    pub workspace: WorkspaceRecord,
    pub work: WorkRecord,
    #[serde(default)]
    pub work_graph: Option<WorkGraphSnapshot>,
    #[serde(default)]
    pub workspace_material: Option<WorkspaceMaterialState>,
    pub pending_deltas: Vec<RemoteDelta>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttachRequest {
    pub session_id: SessionId,
    pub workspace_key: String,
    #[serde(default)]
    pub workspace_aliases: Vec<String>,
    pub repository_identity: String,
    pub work_key: String,
    pub objective: String,
    #[serde(default)]
    pub parent_session_id: Option<SessionId>,
    pub since_revision: Option<u64>,
    #[serde(default)]
    pub material: Option<WorkspaceMaterialObservation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateWorkRequest {
    pub session_id: SessionId,
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub status: Option<WorkStatus>,
    pub since_revision: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReconcileRequest {
    pub session_id: SessionId,
    pub workspace_id: WorkspaceId,
    pub work_id: WorkId,
    pub since_revision: Option<u64>,
    #[serde(default)]
    pub material: Option<WorkspaceMaterialObservation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttachmentSnapshot {
    pub protocol_version: u32,
    pub attachment: SessionAttachment,
    pub state: RuntimeState,
}
