use std::io::BufRead;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::AttachRequest;
use crate::AttachmentSnapshot;
use crate::ReconcileRequest;
use crate::RemoteDelta;
use crate::RuntimeAuthority;
use crate::RuntimeAuthorityError;
use crate::RuntimeState;
use crate::SessionId;
use crate::WorkId;
use crate::WorkspaceId;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", content = "payload")]
enum RuntimeRequest {
    Attach(AttachRequest),
    Reconcile(ReconcileRequest),
    State {
        workspace_id: WorkspaceId,
        work_id: WorkId,
        since_revision: Option<u64>,
    },
    Detach(SessionId),
    RecordRemoteDelta {
        work_id: WorkId,
        summary: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct RuntimeResponse<T> {
    value: Option<T>,
    error: Option<String>,
}

/// Synchronous client for a standalone Brine runtime authority process.
pub struct TcpRuntimeAuthority {
    address: SocketAddr,
}

impl TcpRuntimeAuthority {
    /// Connects to an authority at `address` for one request at a time.
    pub fn new(address: SocketAddr) -> Self {
        Self { address }
    }

    fn request<T: DeserializeOwned>(
        &self,
        request: RuntimeRequest,
    ) -> Result<T, RuntimeAuthorityError> {
        let mut stream = TcpStream::connect(self.address)
            .map_err(|error| RuntimeAuthorityError::Remote(error.to_string()))?;
        let mut payload = serde_json::to_vec(&request)?;
        payload.push(b'\n');
        stream
            .write_all(&payload)
            .map_err(|error| RuntimeAuthorityError::Remote(error.to_string()))?;
        let mut response = String::new();
        std::io::Read::read_to_string(&mut stream, &mut response)
            .map_err(|error| RuntimeAuthorityError::Remote(error.to_string()))?;
        let response: RuntimeResponse<T> = serde_json::from_str(&response)?;
        response.value.ok_or_else(|| {
            RuntimeAuthorityError::Remote(
                response
                    .error
                    .unwrap_or_else(|| "remote authority returned no value".to_owned()),
            )
        })
    }
}

impl RuntimeAuthority for TcpRuntimeAuthority {
    fn attach(&self, request: AttachRequest) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        self.request(RuntimeRequest::Attach(request))
    }

    fn reconcile(
        &self,
        request: ReconcileRequest,
    ) -> Result<AttachmentSnapshot, RuntimeAuthorityError> {
        self.request(RuntimeRequest::Reconcile(request))
    }

    fn state(
        &self,
        workspace_id: &WorkspaceId,
        work_id: &WorkId,
        since_revision: Option<u64>,
    ) -> Result<RuntimeState, RuntimeAuthorityError> {
        self.request(RuntimeRequest::State {
            workspace_id: workspace_id.clone(),
            work_id: work_id.clone(),
            since_revision,
        })
    }

    fn detach(&self, session_id: &SessionId) -> Result<(), RuntimeAuthorityError> {
        self.request(RuntimeRequest::Detach(session_id.clone()))
    }

    fn record_remote_delta(
        &self,
        work_id: &WorkId,
        summary: String,
    ) -> Result<RemoteDelta, RuntimeAuthorityError> {
        self.request(RuntimeRequest::RecordRemoteDelta {
            work_id: work_id.clone(),
            summary,
        })
    }
}

/// A small line-delimited JSON server for a standalone runtime process.
pub struct RuntimeAuthorityServer;

impl RuntimeAuthorityServer {
    /// Serves requests until the listener is closed or `max_connections` is reached.
    pub fn serve(
        listener: TcpListener,
        authority: Arc<dyn RuntimeAuthority>,
        max_connections: Option<usize>,
    ) -> std::io::Result<()> {
        match max_connections {
            Some(limit) => {
                for _ in 0..limit {
                    handle_connection(listener.accept()?.0, Arc::clone(&authority))?;
                }
            }
            None => {
                for stream in listener.incoming() {
                    handle_connection(stream?, Arc::clone(&authority))?;
                }
            }
        }
        Ok(())
    }
}

fn handle_connection(
    mut stream: TcpStream,
    authority: Arc<dyn RuntimeAuthority>,
) -> std::io::Result<()> {
    let mut request_line = String::new();
    std::io::BufReader::new(&mut stream).read_line(&mut request_line)?;
    let response = match serde_json::from_str::<RuntimeRequest>(&request_line) {
        Ok(request) => dispatch(request, authority),
        Err(error) => RuntimeResponse::<serde_json::Value> {
            value: None,
            error: Some(error.to_string()),
        },
    };
    stream.write_all(&serde_json::to_vec(&response)?)?;
    Ok(())
}

fn dispatch(
    request: RuntimeRequest,
    authority: Arc<dyn RuntimeAuthority>,
) -> RuntimeResponse<serde_json::Value> {
    macro_rules! respond {
        ($operation:expr) => {
            match $operation {
                Ok(value) => RuntimeResponse {
                    value: Some(serde_json::to_value(value).expect("wire response serializable")),
                    error: None,
                },
                Err(error) => RuntimeResponse {
                    value: None,
                    error: Some(error.to_string()),
                },
            }
        };
    }

    match request {
        RuntimeRequest::Attach(request) => respond!(authority.attach(request)),
        RuntimeRequest::Reconcile(request) => respond!(authority.reconcile(request)),
        RuntimeRequest::State {
            workspace_id,
            work_id,
            since_revision,
        } => respond!(authority.state(&workspace_id, &work_id, since_revision)),
        RuntimeRequest::Detach(session_id) => respond!(authority.detach(&session_id)),
        RuntimeRequest::RecordRemoteDelta { work_id, summary } => {
            respond!(authority.record_remote_delta(&work_id, summary))
        }
    }
}
