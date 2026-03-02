use crate::node::{Node, NodeBase};

/// VRPN (Virtual-Reality Peripheral Network) output sink.
///
/// There is currently no mature Rust VRPN crate available, so this module
/// defines the struct and interface but marks the start() method as
/// unimplemented. Once a Rust VRPN binding becomes available, this can be
/// filled in.
pub struct VrpnSink {
    pub base: NodeBase,
    m_server_port: u16,
    m_tracker_name: String,
}

impl VrpnSink {
    pub fn new(name: impl Into<String>, server_port: u16, tracker_name: &str) -> Self {
        Self {
            base: NodeBase::new(name),
            m_server_port: server_port,
            m_tracker_name: tracker_name.to_owned(),
        }
    }

    pub fn server_port(&self) -> u16 {
        self.m_server_port
    }

    pub fn tracker_name(&self) -> &str {
        &self.m_tracker_name
    }
}

impl Node for VrpnSink {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        unimplemented!(
            "VrpnSink '{}': No Rust VRPN crate available. \
             VRPN output is not yet supported in the Rust port.",
            self.base.name()
        );
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.base.stop_heartbeat();
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
    fn vrpn_sink_creation() {
        let sink = VrpnSink::new("vrpn_test", 3883, "FusionHub0");
        assert_eq!(sink.name(), "vrpn_test");
        assert_eq!(sink.server_port(), 3883);
        assert_eq!(sink.tracker_name(), "FusionHub0");
    }

    #[test]
    #[should_panic(expected = "No Rust VRPN crate available")]
    fn vrpn_sink_start_panics() {
        let mut sink = VrpnSink::new("vrpn_test", 3883, "FusionHub0");
        let _ = sink.start();
    }
}
