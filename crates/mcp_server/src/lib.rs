mod data_buffer;
mod server;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use rmcp::ServiceExt;
use serde_json::json;
use tokio::sync::{broadcast, RwLock};

use web_ui::ConfigHandle;
use websocket_server::CommandChannel;

use data_buffer::DataBuffer;
use server::{FusionMcpServer, McpStatus};

pub struct McpHandle {
    buffer_task: tokio::task::JoinHandle<()>,
    server_task: tokio::task::JoinHandle<Result<()>>,
    status_task: tokio::task::JoinHandle<()>,
}

impl McpHandle {
    pub async fn wait(self) -> Result<()> {
        self.server_task.await??;
        self.status_task.abort();
        self.buffer_task.abort();
        Ok(())
    }
}

/// Start the MCP server on stdio.
///
/// `real_stdout` is the saved stdout file descriptor.  The caller must have
/// already redirected the process stdout to stderr so that C/C++ libraries
/// cannot contaminate the MCP JSON-RPC transport.
pub async fn start_stdio(
    config_handle: ConfigHandle,
    sse_tx: broadcast::Sender<String>,
    command_channel: CommandChannel,
    paused: Arc<AtomicBool>,
    real_stdout: std::fs::File,
) -> Result<McpHandle> {
    let buffer = Arc::new(RwLock::new(DataBuffer::new()));
    let buffer_task = data_buffer::spawn_ingestion_task(&sse_tx, buffer.clone());

    let status = Arc::new(RwLock::new(McpStatus::default()));
    let mcp = FusionMcpServer::new(buffer, config_handle, command_channel, paused, status.clone());

    // Spawn periodic status broadcast (every 2 seconds)
    let status_for_broadcast = status.clone();
    let sse_tx_clone = sse_tx.clone();
    let status_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            let s = status_for_broadcast.read().await;
            let msg = json!({ "type": "mcpStatus", "data": *s });
            let _ = sse_tx_clone.send(msg.to_string());
        }
    });

    let status_for_lifecycle = status.clone();
    let server_task = tokio::spawn(async move {
        // Use the saved stdout for MCP transport (process stdout is now stderr)
        let stdin = tokio::io::stdin();
        let stdout = tokio::fs::File::from_std(real_stdout);
        let transport = (stdin, stdout);

        let service = mcp
            .serve(transport)
            .await
            .map_err(|e| anyhow::anyhow!("MCP server init failed: {}", e))?;

        // Mark connected after successful handshake
        {
            let mut s = status_for_lifecycle.write().await;
            s.connected = true;
            s.connected_since = Some(Utc::now().to_rfc3339());
            s.client_name = Some("MCP Client".into());
        }
        log::info!("MCP client connected");

        service
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("MCP server error: {}", e))?;

        // Mark disconnected
        {
            let mut s = status_for_lifecycle.write().await;
            s.connected = false;
        }
        log::info!("MCP client disconnected");

        Ok(())
    });

    Ok(McpHandle {
        buffer_task,
        server_task,
        status_task,
    })
}
