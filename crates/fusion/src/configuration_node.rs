use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::node::{Node, NodeBase};

/// Callback invoked when configuration changes.
pub type ConfigChangeCallback = Box<dyn Fn(&Value) + Send + Sync>;

/// Manages runtime configuration (load/save JSON).
/// Notifies registered listeners on configuration changes.
pub struct ConfigurationNode {
    pub base: NodeBase,
    m_config: Arc<Mutex<Value>>,
    m_file_path: PathBuf,
    m_change_callbacks: Arc<Mutex<Vec<ConfigChangeCallback>>>,
}

impl ConfigurationNode {
    pub fn new(name: impl Into<String>, file_path: impl AsRef<Path>) -> Self {
        Self {
            base: NodeBase::new(name),
            m_config: Arc::new(Mutex::new(Value::Object(serde_json::Map::new()))),
            m_file_path: file_path.as_ref().to_path_buf(),
            m_change_callbacks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Load configuration from the JSON file.
    pub fn load(&self) -> anyhow::Result<()> {
        let contents = std::fs::read_to_string(&self.m_file_path)?;
        let value: Value = serde_json::from_str(&contents)?;
        let mut config = self.m_config.lock().unwrap();
        *config = value;
        drop(config);
        self.notify_change();
        log::info!(
            "ConfigurationNode '{}': loaded from '{}'",
            self.base.name(),
            self.m_file_path.display()
        );
        Ok(())
    }

    /// Save current configuration to the JSON file.
    pub fn save(&self) -> anyhow::Result<()> {
        let config = self.m_config.lock().unwrap();
        let contents = serde_json::to_string_pretty(&*config)?;
        drop(config);
        std::fs::write(&self.m_file_path, contents)?;
        log::info!(
            "ConfigurationNode '{}': saved to '{}'",
            self.base.name(),
            self.m_file_path.display()
        );
        Ok(())
    }

    /// Get the full configuration as JSON.
    pub fn config(&self) -> Value {
        self.m_config.lock().unwrap().clone()
    }

    /// Get a specific configuration value by JSON pointer path.
    pub fn get(&self, pointer: &str) -> Option<Value> {
        let config = self.m_config.lock().unwrap();
        config.pointer(pointer).cloned()
    }

    /// Set a specific value by key.
    pub fn set(&self, key: &str, value: Value) {
        let mut config = self.m_config.lock().unwrap();
        if let Value::Object(ref mut map) = *config {
            map.insert(key.to_owned(), value);
        }
        drop(config);
        self.notify_change();
    }

    /// Merge a JSON object into the current configuration.
    pub fn merge(&self, patch: &Value) {
        let mut config = self.m_config.lock().unwrap();
        if let (Value::Object(ref mut base), Value::Object(ref incoming)) = (&mut *config, patch) {
            for (k, v) in incoming {
                base.insert(k.clone(), v.clone());
            }
        }
        drop(config);
        self.notify_change();
    }

    /// Register a callback for configuration changes.
    pub fn on_change(&self, callback: ConfigChangeCallback) {
        let mut callbacks = self.m_change_callbacks.lock().unwrap();
        callbacks.push(callback);
    }

    fn notify_change(&self) {
        let config = self.m_config.lock().unwrap().clone();
        let callbacks = self.m_change_callbacks.lock().unwrap();
        for cb in callbacks.iter() {
            cb(&config);
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.m_file_path
    }
}

impl Node for ConfigurationNode {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.m_file_path.exists() {
            self.load()?;
        } else {
            log::info!(
                "ConfigurationNode '{}': config file not found, using defaults",
                self.base.name()
            );
        }
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.save().ok();
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_set_get() {
        let node = ConfigurationNode::new("cfg", "/tmp/nonexistent.json");
        node.set("key1", serde_json::json!("value1"));
        let val = node.get("/key1");
        assert_eq!(val.unwrap(), serde_json::json!("value1"));
    }

    #[test]
    fn config_merge() {
        let node = ConfigurationNode::new("cfg", "/tmp/nonexistent.json");
        node.set("a", serde_json::json!(1));
        node.merge(&serde_json::json!({"b": 2, "c": 3}));
        assert_eq!(node.get("/a").unwrap(), serde_json::json!(1));
        assert_eq!(node.get("/b").unwrap(), serde_json::json!(2));
        assert_eq!(node.get("/c").unwrap(), serde_json::json!(3));
    }

    #[test]
    fn config_change_notification() {
        let node = ConfigurationNode::new("cfg", "/tmp/nonexistent.json");
        let counter = Arc::new(Mutex::new(0usize));
        let c = counter.clone();
        node.on_change(Box::new(move |_| {
            *c.lock().unwrap() += 1;
        }));
        node.set("x", serde_json::json!(42));
        assert_eq!(*counter.lock().unwrap(), 1);
    }
}
