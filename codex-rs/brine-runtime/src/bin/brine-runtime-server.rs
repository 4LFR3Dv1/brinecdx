use std::net::SocketAddr;
use std::net::TcpListener;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use codex_brine_runtime::FileRuntimeAuthority;
use codex_brine_runtime::RuntimeAuthorityServer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let state_path = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: brine-runtime-server <state-path> [bind-address]")?;
    let address = args
        .next()
        .map(|value| SocketAddr::from_str(&value))
        .transpose()?
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 4545)));
    let authority = Arc::new(FileRuntimeAuthority::open(state_path)?);
    let listener = TcpListener::bind(address)?;
    eprintln!(
        "brine runtime authority listening on {}",
        listener.local_addr()?
    );
    RuntimeAuthorityServer::serve(listener, authority, None)?;
    Ok(())
}
