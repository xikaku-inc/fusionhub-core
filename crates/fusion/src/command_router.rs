use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fusion_types::ApiRequest;

/// Handler function for API commands.
pub type CommandHandler = Box<dyn Fn(&ApiRequest) -> Option<ApiRequest> + Send + Sync>;

/// Routes ApiRequest messages to registered handlers by command name.
pub struct CommandRouter {
    m_node_name: String,
    m_handlers: Arc<Mutex<HashMap<String, CommandHandler>>>,
    m_default_handler: Arc<Mutex<Option<CommandHandler>>>,
}

impl CommandRouter {
    pub fn new(node_name: impl Into<String>) -> Self {
        Self {
            m_node_name: node_name.into(),
            m_handlers: Arc::new(Mutex::new(HashMap::new())),
            m_default_handler: Arc::new(Mutex::new(None)),
        }
    }

    pub fn node_name(&self) -> &str {
        &self.m_node_name
    }

    /// Register a handler for a specific command name.
    pub fn register(&self, command: impl Into<String>, handler: CommandHandler) {
        let mut handlers = self.m_handlers.lock().unwrap();
        handlers.insert(command.into(), handler);
    }

    /// Set a default handler for unmatched commands.
    pub fn set_default_handler(&self, handler: CommandHandler) {
        let mut default = self.m_default_handler.lock().unwrap();
        *default = Some(handler);
    }

    /// Route a request to the appropriate handler.
    /// Returns a response ApiRequest if the handler produces one.
    pub fn route(&self, request: &ApiRequest) -> Option<ApiRequest> {
        if !request.topic.is_empty() && request.topic != self.m_node_name {
            return None;
        }

        let handlers = self.m_handlers.lock().unwrap();
        if let Some(handler) = handlers.get(&request.command) {
            return handler(request);
        }
        drop(handlers);

        let default = self.m_default_handler.lock().unwrap();
        if let Some(handler) = default.as_ref() {
            return handler(request);
        }

        log::trace!(
            "CommandRouter '{}': no handler for command '{}'",
            self.m_node_name,
            request.command
        );
        None
    }

    /// Check if a handler exists for the given command.
    pub fn has_handler(&self, command: &str) -> bool {
        self.m_handlers.lock().unwrap().contains_key(command)
    }

    /// List all registered command names.
    pub fn commands(&self) -> Vec<String> {
        self.m_handlers
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_to_handler() {
        let router = CommandRouter::new("imu0");
        router.register(
            "getStatus",
            Box::new(|req| {
                Some(ApiRequest::new(
                    "statusResponse",
                    &req.topic,
                    serde_json::json!({"status": "ok"}),
                    &req.id,
                ))
            }),
        );

        let req = ApiRequest::new("getStatus", "imu0", serde_json::Value::Null, "1");
        let resp = router.route(&req);
        assert!(resp.is_some());
        assert_eq!(resp.unwrap().command, "statusResponse");
    }

    #[test]
    fn route_wrong_topic() {
        let router = CommandRouter::new("imu0");
        router.register("getStatus", Box::new(|_| None));

        let req = ApiRequest::new("getStatus", "gnss0", serde_json::Value::Null, "1");
        let resp = router.route(&req);
        assert!(resp.is_none());
    }

    #[test]
    fn default_handler() {
        let router = CommandRouter::new("node");
        router.set_default_handler(Box::new(|req| {
            Some(ApiRequest::new(
                "unknown",
                &req.topic,
                serde_json::Value::Null,
                &req.id,
            ))
        }));

        let req = ApiRequest::new("anyCommand", "node", serde_json::Value::Null, "2");
        let resp = router.route(&req);
        assert!(resp.is_some());
        assert_eq!(resp.unwrap().command, "unknown");
    }
}
