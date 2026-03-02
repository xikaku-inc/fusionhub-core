use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::broadcast;

use fusion_types::{ApiRequest, StreamableData};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Registry mapping original endpoint names to their resolved TCP addresses
/// after a Publisher binds. Used only for TCP endpoints.
static ENDPOINT_REGISTRY: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// Broadcast channel registries for inproc:// endpoints.
static DATA_CHANNEL_REGISTRY: OnceLock<Mutex<HashMap<String, broadcast::Sender<Arc<StreamableData>>>>> =
    OnceLock::new();
static COMMAND_CHANNEL_REGISTRY: OnceLock<Mutex<HashMap<String, broadcast::Sender<ApiRequest>>>> =
    OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, String>> {
    ENDPOINT_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn data_channel_registry() -> &'static Mutex<HashMap<String, broadcast::Sender<Arc<StreamableData>>>> {
    DATA_CHANNEL_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn command_channel_registry() -> &'static Mutex<HashMap<String, broadcast::Sender<ApiRequest>>> {
    COMMAND_CHANNEL_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn is_inproc(endpoint: &str) -> bool {
    endpoint.starts_with("inproc://")
}

/// Register a resolved endpoint for an original endpoint name.
/// Called by Publisher after binding to record the actual address.
pub fn register_endpoint(original: &str, resolved: &str) {
    let mut reg = registry().lock().unwrap();
    log::debug!("Endpoint registry: '{}' -> '{}'", original, resolved);
    reg.insert(original.to_owned(), resolved.to_owned());
}

/// Look up the resolved endpoint for a given original name.
pub fn lookup_endpoint(original: &str) -> Option<String> {
    let reg = registry().lock().unwrap();
    reg.get(original).cloned()
}

pub fn register_data_channel(name: &str, sender: broadcast::Sender<Arc<StreamableData>>) {
    let mut reg = data_channel_registry().lock().unwrap();
    log::debug!("Data channel registry: '{}'", name);
    reg.insert(name.to_owned(), sender);
}

pub fn get_data_channel(name: &str) -> Option<broadcast::Sender<Arc<StreamableData>>> {
    let reg = data_channel_registry().lock().unwrap();
    reg.get(name).cloned()
}

pub fn register_command_channel(name: &str, sender: broadcast::Sender<ApiRequest>) {
    let mut reg = command_channel_registry().lock().unwrap();
    log::debug!("Command channel registry: '{}'", name);
    reg.insert(name.to_owned(), sender);
}

pub fn get_command_channel(name: &str) -> Option<broadcast::Sender<ApiRequest>> {
    let reg = command_channel_registry().lock().unwrap();
    reg.get(name).cloned()
}

/// Get a handle to the dedicated ZMQ tokio runtime.
///
/// Always uses a separate runtime to avoid deadlocks when Publisher/Subscriber
/// constructors block-wait for async operations to complete.
pub(crate) fn runtime_handle() -> Handle {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .thread_name("zmq-runtime")
                .build()
                .expect("Failed to create ZMQ tokio runtime")
        })
        .handle()
        .clone()
}

/// Resolve an endpoint for binding (Publisher/CommandPublisher).
/// Only called for TCP endpoints.
/// - tcp://*:PORT → tcp://0.0.0.0:PORT (bind-compatible)
pub(crate) fn resolve_endpoint_for_bind(endpoint: &str) -> String {
    endpoint.replace("tcp://*:", "tcp://0.0.0.0:")
}

/// Resolve an endpoint for connecting (Subscriber/CommandSubscriber).
/// Only called for TCP endpoints.
pub(crate) fn resolve_endpoint_for_connect(endpoint: &str) -> String {
    if endpoint.starts_with("inproc://") {
        return endpoint.to_owned();
    }

    if let Some(resolved) = lookup_endpoint(endpoint) {
        return star_to_localhost_single(&resolved);
    }

    if let Some(port) = extract_tcp_port(endpoint) {
        let wildcard_form = format!("tcp://*:{}", port);
        if let Some(resolved) = lookup_endpoint(&wildcard_form) {
            return star_to_localhost_single(&resolved);
        }
        let zero_form = format!("tcp://0.0.0.0:{}", port);
        if let Some(resolved) = lookup_endpoint(&zero_form) {
            return star_to_localhost_single(&resolved);
        }
    }

    star_to_localhost_single(endpoint)
}

fn extract_tcp_port(ep: &str) -> Option<&str> {
    ep.strip_prefix("tcp://").and_then(|rest| rest.rsplit(':').next())
}

fn star_to_localhost_single(ep: &str) -> String {
    ep.replace("tcp://*:", "tcp://127.0.0.1:")
        .replace("tcp://0.0.0.0:", "tcp://127.0.0.1:")
        .replace("tcp://localhost:", "tcp://127.0.0.1:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_inproc() {
        assert!(is_inproc("inproc://test"));
        assert!(!is_inproc("tcp://127.0.0.1:5555"));
    }

    #[test]
    fn resolve_bind_wildcard() {
        let resolved = resolve_endpoint_for_bind("tcp://*:8799");
        assert_eq!(resolved, "tcp://0.0.0.0:8799");
    }

    #[test]
    fn resolve_bind_tcp_unchanged() {
        let resolved = resolve_endpoint_for_bind("tcp://127.0.0.1:5555");
        assert_eq!(resolved, "tcp://127.0.0.1:5555");
    }

    #[test]
    fn resolve_connect_wildcard() {
        let resolved = resolve_endpoint_for_connect("tcp://*:8799");
        assert_eq!(resolved, "tcp://127.0.0.1:8799");
    }

    #[test]
    fn resolve_connect_registry() {
        register_endpoint("tcp://*:55555", "tcp://0.0.0.0:55555");
        let resolved = resolve_endpoint_for_connect("tcp://localhost:55555");
        assert_eq!(resolved, "tcp://127.0.0.1:55555");
    }

    #[test]
    fn resolve_connect_tcp_unchanged() {
        let resolved = resolve_endpoint_for_connect("tcp://127.0.0.1:5555");
        assert_eq!(resolved, "tcp://127.0.0.1:5555");
    }

    #[test]
    fn resolve_connect_inproc_passthrough() {
        let resolved = resolve_endpoint_for_connect("inproc://test_data");
        assert_eq!(resolved, "inproc://test_data");
    }

    #[test]
    fn data_channel_registry_roundtrip() {
        let (sender, _) = broadcast::channel::<Arc<StreamableData>>(16);
        register_data_channel("inproc://test_data_reg", sender);
        assert!(get_data_channel("inproc://test_data_reg").is_some());
        assert!(get_data_channel("inproc://nonexistent").is_none());
    }

    #[test]
    fn command_channel_registry_roundtrip() {
        let (sender, _) = broadcast::channel::<ApiRequest>(16);
        register_command_channel("inproc://test_cmd_reg", sender);
        assert!(get_command_channel("inproc://test_cmd_reg").is_some());
        assert!(get_command_channel("inproc://nonexistent").is_none());
    }
}
