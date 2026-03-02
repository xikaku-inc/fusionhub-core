#![allow(dead_code)]

mod runtime;
mod publisher;
mod subscriber;
mod command;
mod network_writer;
mod disk;

pub use publisher::Publisher;
pub use subscriber::Subscriber;
pub use command::{CommandPublisher, CommandSubscriber};
pub use network_writer::NetworkWriter;
pub use disk::{DiskWriter, DiskReader};

/// Converts endpoint strings for subscriber connections.
/// Replaces `*` and `0.0.0.0` with `localhost`, mirroring C++ `EndpointStringConverter::starToLocalhost`.
pub fn star_to_localhost(endpoints: &[String]) -> Vec<String> {
    endpoints
        .iter()
        .map(|ep| {
            let converted = ep.replace("0.0.0.0", "localhost");
            converted.replace("*", "localhost")
        })
        .collect()
}
