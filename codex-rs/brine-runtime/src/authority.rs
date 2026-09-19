use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::AttachRequest;
use crate::AttachmentSnapshot;
use crate::ReconcileRequest;
use crate::RemoteDelta;
use crate::RuntimeState;
use crate::SessionAttachment;
use crate::SessionId;
use crate::WorkId;
use crate::WorkRecord;
use crate::WorkspaceId;
use crate::WorkspaceMaterialObservation;
use crate::WorkspaceMaterialState;
use crate::WorkspaceRecord;
use crate::WorkspaceRevisionRecord;

const ROOT_WORK_KEY: &str = "root";

/// Errors returned by a Brine runtime authority.
#[derive(Debug, Error)]
pub enum RuntimeAuthorityError {
    #[error("runtime state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("runtime state serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("workspace {0} was not found")]
    WorkspaceNotFound(WorkspaceId),
    #[error("work {0} was not found")]
    WorkNotFound(WorkId),
    #[error("session {0} is not attached to work {1}")]
    SessionNotAttached(SessionId, WorkId),
    #[error("remote runtime authority failed: {0}")]
    Remote(String),
}

/// Durable logical authority used by a Brine session.
///
/// Implementations must persist workspace and work identity independently of
/// any Codex process. Methods intentionally accept no shell or filesystem
/// handles: physical reality is observed by the attaching host.
pub trait RuntimeAuthority: Send + Sync {
    /// Attach a temporary session to durable workspace and work identities.
    ///
    /// An empty work key selects the workspace's stable root Work.
    fn attach(&self, request: AttachRequest) -> Result<AttachmentSnapshot, RuntimeAuthorityError>;

    /// Reconcile an existing attachment and return remote deltas since a revision.
    fn reconcile(
        &self,
        request: ReconcileRequest,
    ) -> Result<AttachmentSnapshot, RuntimeAuthorityError>;

    /// Reads durable logical state without requiring a live session.
    fn state(
        &self,
        workspace_id: &WorkspaceId,
        work_id: &WorkId,
        since_revision: Option<u64>,
    ) -> Result<RuntimeState, RuntimeAuthorityError>;

    /// Detach a session without deleting its workspace or work.
    fn detach(&self, session_id: &SessionId) -> Result<(), RuntimeAuthorityError>;

    /// Record a logical event while no Codex session is attached.
    fn record_remote_delta(
        &self,
        work_id: &WorkId,
        summary: String,
    ) -> Result<RemoteDelta, RuntimeAuthorityError>;
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PersistedRuntimeState {
    revision: u64,
    workspaces: BTreeMap<String, WorkspaceRecord>,
    works: BTreeMap<String, WorkRecord>,
    attachments: BTreeMap<String, SessionAttachment>,
    deltas: Vec<RemoteDelta>,
    #[serde(default)]
    workspace_material: BTreeMap<String, WorkspaceMaterialState>,
    #[serde(default)]
    workspace_revisions: Vec<WorkspaceRevisionRecord>,
}

impl PersistedRuntimeState {
    fn snapshot(
        &self,
        workspace_id: &WorkspaceId,
        work_id: &WorkId,
        since_revision: Option<u64>,
    ) -> Result<RuntimeState, RuntimeAuthorityError> {
        let workspace = self
            .workspaces
            .get(workspace_id.as_str())
            .cloned()
            .ok_or_else(|| RuntimeAuthorityError::WorkspaceNotFound(workspace_id.clone()))?;
        let work = self
            .works
            .get(work_id.as_str())
            .cloned()
            .ok_or_else(|| RuntimeAuthorityError::WorkNotFound(work_id.clone()))?;
        let minimum_revision = since_revision.unwrap_or(0);
        let pending_deltas = self
            .deltas
            .iter()
            .filter(|delta| delta.work_id == *work_id && delta.revision > minimum_revision)
            .cloned()
            .collect();
        Ok(RuntimeState {
            revision: self.revision,
            workspace,
            work,
            workspace_material: self.workspace_material.get(workspace_id.as_str()).cloned(),
            pending_deltas,
        })
    }

    fn attach(
        &mut self,
        request: &AttachRequest,
    ) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        let work_key = if request.work_key.is_empty() {
            ROOT_WORK_KEY.to_owned()
        } else {
            request.work_key.clone()
        };
        let exact_workspace_id = self
            .workspaces
            .values()
            .find(|workspace| workspace.key == request.workspace_key)
            .map(|workspace| workspace.id.clone());
        let aliased_workspace_id = exact_workspace_id.is_none().then(|| {
            self.workspaces
                .values()
                .find(|workspace| {
                    request
                        .workspace_aliases
                        .iter()
                        .any(|alias| alias == &workspace.key)
                })
                .map(|workspace| workspace.id.clone())
        }).flatten();

        let workspace_id = match exact_workspace_id.or(aliased_workspace_id) {
            Some(id) => {
                let needs_identity_update = self
                    .workspaces
                    .get(id.as_str())
                    .is_some_and(|workspace| {
                        workspace.key != request.workspace_key
                            || workspace.repository_identity != request.repository_identity
                    });
                if needs_identity_update {
                    self.revision += 1;
                    let workspace = self
                        .workspaces
                        .get_mut(id.as_str())
                        .expect("resolved workspace must remain present");
                    workspace.key = request.workspace_key.clone();
                    workspace.repository_identity = request.repository_identity.clone();
                }
                id
            }
            None => {
                let id = WorkspaceId::new();
                self.revision += 1;
                self.workspaces.insert(
                    id.to_string(),
                    WorkspaceRecord {
                        id: id.clone(),
                        key: request.workspace_key.clone(),
                        repository_identity: request.repository_identity.clone(),
                        created_revision: self.revision,
                    },
                );
                id
            }
        };

        self.apply_workspace_material(&workspace_id, request.material.as_ref());

        let work_id = self
            .works
            .values()
            .find(|work| work.workspace_id == workspace_id && work.key == work_key)
            .map(|work| work.id.clone())
            .unwrap_or_else(|| {
                let id = WorkId::new();
                self.revision += 1;
                self.works.insert(
                    id.to_string(),
                    WorkRecord {
                        id: id.clone(),
                        workspace_id: workspace_id.clone(),
                        key: work_key.clone(),
                        objective: request.objective.clone(),
                        created_revision: self.revision,
                        last_revision: self.revision,
                    },
                );
                id
            });

        let attachment = SessionAttachment {
            session_id: request.session_id.clone(),
            workspace_id: workspace_id.clone(),
            work_id: work_id.clone(),
            attached_revision: self.revision,
        };
        self.attachments
            .insert(request.session_id.to_string(), attachment.clone());
        Ok(AttachmentSnapshot {
            state: self.snapshot(&workspace_id, &work_id, request.since_revision)?,
            attachment,
        })
    }

    fn reconcile(
        &mut self,
        request: &ReconcileRequest,
    ) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        let Some(existing_attachment) = self
            .attachments
            .get(request.session_id.as_str())
            .cloned()
        else {
            return Err(RuntimeAuthorityError::SessionNotAttached(
                request.session_id.clone(),
                request.work_id.clone(),
            ));
        };
        if existing_attachment.workspace_id != request.workspace_id
            || existing_attachment.work_id != request.work_id
        {
            return Err(RuntimeAuthorityError::SessionNotAttached(
                request.session_id.clone(),
                request.work_id.clone(),
            ));
        }

        self.apply_workspace_material(&request.workspace_id, request.material.as_ref());

        let attachment = self
            .attachments
            .get_mut(request.session_id.as_str())
            .expect("validated attachment remains present");
        attachment.attached_revision = self.revision;
        let attachment = attachment.clone();
        Ok(AttachmentSnapshot {
            state: self.snapshot(
                &request.workspace_id,
                &request.work_id,
                request.since_revision,
            )?,
            attachment,
        })
    }

    fn apply_workspace_material(
        &mut self,
        workspace_id: &WorkspaceId,
        observation: Option<&WorkspaceMaterialObservation>,
    ) {
        let Some(observation) = observation else {
            return;
        };
        if self
            .workspace_material
            .get(workspace_id.as_str())
            .is_some_and(|state| state.observation.material_digest == observation.material_digest)
        {
            return;
        }

        let previous = self.workspace_material.get(workspace_id.as_str());
        let material_revision = previous
            .map(|state| state.material_revision + 1)
            .unwrap_or(1);
        let structure_digest = &observation.structure.digest;
        let structural_revision = if structure_digest.is_empty() {
            previous.map(|state| state.structural_revision).unwrap_or(0)
        } else if previous.is_some_and(|state| {
            state.observation.structure.digest == *structure_digest
        }) {
            previous
                .map(|state| state.structural_revision.max(1))
                .unwrap_or(1)
        } else {
            previous
                .map(|state| state.structural_revision.saturating_add(1).max(1))
                .unwrap_or(1)
        };

        self.revision += 1;
        let runtime_revision = self.revision;
        let material_state = WorkspaceMaterialState {
            workspace_id: workspace_id.clone(),
            material_revision,
            structural_revision,
            runtime_revision,
            observation: observation.clone(),
        };
        self.workspace_material
            .insert(workspace_id.to_string(), material_state);
        self.workspace_revisions.push(WorkspaceRevisionRecord {
            workspace_id: workspace_id.clone(),
            material_revision,
            structural_revision,
            runtime_revision,
            material_digest: observation.material_digest.clone(),
            structure_digest: observation.structure.digest.clone(),
            head: observation.head.clone(),
            changed_paths: observation.changed_paths.clone(),
        });
    }
}

/// A file-backed authority suitable for a remote runtime process.
///
/// The file contains only logical runtime state. A Codex session may be
/// destroyed and recreated while this authority remains on disk.
pub struct FileRuntimeAuthority {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileRuntimeAuthority {
    /// Opens or creates an authority at `path`.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, RuntimeAuthorityError> {
        let path = path.into();
        if path.exists() {
            let _ = read_state(&path)?;
        }
        Ok(Self {
            path,
            lock: Mutex::new(()),
        })
    }

    /// Returns the path containing durable logical state.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn update<T>(
        &self,
        operation: impl FnOnce(&mut PersistedRuntimeState) -> Result<T, RuntimeAuthorityError>,
    ) -> Result<T, RuntimeAuthorityError> {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let mut state = read_state(&self.path)?;
        let result = operation(&mut state)?;
        write_state(&self.path, &state)?;
        Ok(result)
    }
}

impl RuntimeAuthority for FileRuntimeAuthority {
    fn attach(&self, request: AttachRequest) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        self.update(|state| state.attach(&request))
    }

    fn reconcile(
        &self,
        request: ReconcileRequest,
    ) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        self.update(|state| state.reconcile(&request))
    }

    fn state(
        &self,
        workspace_id: &WorkspaceId,
        work_id: &WorkId,
        since_revision: Option<u64>,
    ) -> Result<RuntimeState, RuntimeAuthorityError> {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        read_state(&self.path)?.snapshot(workspace_id, work_id, since_revision)
    }

    fn detach(&self, session_id: &SessionId) -> Result<(), RuntimeAuthorityError> {
        self.update(|state| {
            state.attachments.remove(session_id.as_str());
            Ok(())
        })
    }

    fn record_remote_delta(
        &self,
        work_id: &WorkId,
        summary: String,
    ) -> Result<RemoteDelta, RuntimeAuthorityError> {
        self.update(|state| record_delta(state, work_id, summary))
    }
}

/// An in-memory authority used for host integration tests and embedders.
#[derive(Clone, Default)]
pub struct InMemoryRuntimeAuthority {
    state: Arc<Mutex<PersistedRuntimeState>>,
}

impl RuntimeAuthority for InMemoryRuntimeAuthority {
    fn attach(&self, request: AttachRequest) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.attach(&request)
    }

    fn reconcile(
        &self,
        request: ReconcileRequest,
    ) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.reconcile(&request)
    }

    fn state(
        &self,
        workspace_id: &WorkspaceId,
        work_id: &WorkId,
        since_revision: Option<u64>,
    ) -> Result<RuntimeState, RuntimeAuthorityError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.snapshot(workspace_id, work_id, since_revision)
    }

    fn detach(&self, session_id: &SessionId) -> Result<(), RuntimeAuthorityError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.attachments.remove(session_id.as_str());
        Ok(())
    }

    fn record_remote_delta(
        &self,
        work_id: &WorkId,
        summary: String,
    ) -> Result<RemoteDelta, RuntimeAuthorityError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        record_delta(&mut state, work_id, summary)
    }
}

fn record_delta(
    state: &mut PersistedRuntimeState,
    work_id: &WorkId,
    summary: String,
) -> Result<RemoteDelta, RuntimeAuthorityError> {
    let next_revision = state.revision + 1;
    let Some(work) = state.works.get_mut(work_id.as_str()) else {
        return Err(RuntimeAuthorityError::WorkNotFound(work_id.clone()));
    };
    work.last_revision = next_revision;
    state.revision = next_revision;
    let delta = RemoteDelta {
        revision: state.revision,
        work_id: work_id.clone(),
        summary,
    };
    state.deltas.push(delta.clone());
    Ok(delta)
}

fn read_state(path: &Path) -> Result<PersistedRuntimeState, RuntimeAuthorityError> {
    if !path.exists() {
        return Ok(PersistedRuntimeState::default());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn write_state(path: &Path, state: &PersistedRuntimeState) -> Result<(), RuntimeAuthorityError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}
