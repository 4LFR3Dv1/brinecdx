use std::time::Duration;

use codex_exec_server::EnvironmentManager;
use codex_exec_server::EnvironmentObservedStatus;
use codex_exec_server::REMOTE_ENVIRONMENT_ID;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use tokio::time::Instant;
use tokio::time::sleep;

const WITNESS_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let http_client_factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let manager = EnvironmentManager::from_env(None, http_client_factory).await?;

    if manager.default_environment_id() != Some(REMOTE_ENVIRONMENT_ID) {
        return Err("Brine witness expected the remote exec-server to be the default environment".into());
    }

    let deadline = Instant::now() + WITNESS_TIMEOUT;
    loop {
        match manager.get_environment_status(REMOTE_ENVIRONMENT_ID).await {
            Some(EnvironmentObservedStatus::Ready) => {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "brinecdx.exec-server-live-witness",
                        "schemaVersion": 1,
                        "environmentId": REMOTE_ENVIRONMENT_ID,
                        "status": "ready",
                        "transport": "websocket",
                        "authenticated": true
                    })
                );
                return Ok(());
            }
            Some(EnvironmentObservedStatus::Disconnected { error }) => {
                return Err(format!("Brine exec-server disconnected during live witness: {error}").into());
            }
            Some(EnvironmentObservedStatus::Pending) => {}
            None => return Err("Brine remote environment disappeared during live witness".into()),
        }

        if Instant::now() >= deadline {
            return Err("Brine exec-server live witness timed out before Ready".into());
        }
        sleep(POLL_INTERVAL).await;
    }
}
