# FusionHub Core

Open-source multi-sensor fusion framework written in Rust. FusionHub Core provides the complete runtime, plugin system, web UI, and networking layer for building real-time sensor fusion applications. It can run standalone or be extended with proprietary node implementations.

## Features

- **Plugin-based node registry** — Global registry with self-registration pattern; add new sources, filters, and sinks without modifying core code
- **UI extension system** — Micro-frontend architecture for pluggable web UI pages
- **Directed node graph** — Sources, filters, and sinks connected via ZMQ pub/sub with in-process or TCP transport
- **Web-based control** — Embedded React UI with real-time dashboard, visual node editor, and live monitoring
- **WebSocket API** — Real-time data streaming and remote command interface
- **Hot-reload** — Restart the node graph without restarting the process
- **Protocol Buffers** — Compact, language-agnostic data serialization
- **C FFI bindings** — Embed FusionHub as a library in C/C++ applications

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    FusionHub Core                         │
│                                                          │
│    Sources          Filters             Sinks            │
│  ┌──────────┐   ┌───────────────┐   ┌─────────────┐     │
│  │ OpenZen  │   │ Prediction    │   │ WebSocket   │     │
│  │ DTrack   │   │ InsideOut     │   │ MQTT        │     │
│  │ NMEA     │-->│ Intercalib.   │-->│ FileLogger  │     │
│  │ MQTT     │-->│ (+ any        │-->│ Echo        │     │
│  │ CAN Bus  │   │  registered)  │   │ DTrack Out  │     │
│  │ Serial   │   │               │   │ ROS2 / VRPN │     │
│  └──────────┘   └───────────────┘   └─────────────┘     │
│                                                          │
│  ┌──────────────────┐  ┌──────────────────────────────┐  │
│  │ WebSocket Server │  │ Web UI (Axum HTTP + SSE)     │  │
│  │ :19358           │  │ :19359                       │  │
│  └──────────────────┘  └──────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

## Project Structure

```
fusionhub-core/
├── src/
│   ├── main.rs                    # Core binary: CLI, FusionHub orchestration
│   └── wiring.rs                  # Node connection & ZMQ network wiring
├── crates/
│   ├── fusion_registry/           # Global node registry (Node trait, metadata, factories)
│   ├── fusion/                    # Node implementations, factory, core registrations
│   ├── fusion_types/              # Core data types (ImuData, GnssData, FusedPose, etc.)
│   ├── fusion_protobuf/           # Protocol Buffer definitions & codec
│   ├── networking/                # ZMQ-based pub/sub & command messaging
│   ├── web_ui/                    # Axum HTTP server with embedded React UI
│   │   └── frontend/              # React + TypeScript + Vite frontend
│   ├── websocket_server/          # WebSocket API server
│   ├── crypto/                    # License validation (SUSI)
│   ├── capi/                      # C-compatible FFI bindings
│   ├── openzen-sys/               # OpenZen C++ SDK wrapper (build-time)
│   ├── recorder/                  # Data recording tool
│   ├── replay/                    # Data replay tool
│   ├── lockit/                    # Timecode decoding
│   └── log_utils/                 # Logging configuration
└── external/
    └── susi/                      # License client (git submodule)
```

## Node Registry

The node registry (`fusion_registry`) is the backbone of the plugin architecture. Node types self-register their metadata and builder functions into a global, thread-safe registry at startup.

### Node Trait

```rust
pub trait Node: Send + Sync {
    fn name(&self) -> &str;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn is_enabled(&self) -> bool;
    fn set_enabled(&mut self, enabled: bool);
    fn receive_data(&mut self, _data: StreamableData) {}
    fn receive_command(&mut self, _cmd: &ApiRequest) {}
    fn set_on_output(&self, _callback: ConsumerCallback) {}
    fn set_on_command_output(&self, _callback: CommandConsumerCallback) {}
    fn status(&self) -> Value { Value::Null }
}
```

### NodeMetadata

```rust
pub struct NodeMetadata {
    pub id: String,                          // Primary identifier
    pub display_name: String,                // Human-readable name
    pub role: NodeRole,                      // Source, Filter, or Sink
    pub config_aliases: Vec<String>,         // Alternative config key names
    pub inputs: Vec<String>,                 // Accepted data types
    pub outputs: Vec<String>,                // Emitted data types
    pub default_settings: Value,             // Default JSON settings
    pub settings_schema: Vec<SettingsField>, // UI form schema for the node editor
    pub subtypes: Option<Vec<NodeSubtype>>,  // Optional variant subtypes
    pub required_feature: Option<String>,    // License feature gate
    pub supports_realtime_config: bool,      // Live config update support
    pub color: String,                       // UI color for the node editor
}
```

### Registering Custom Nodes

External crates register nodes by calling `register_node()` before the main loop starts:

```rust
use fusion_registry::{register_node, NodeMetadata, NodeRole};

pub fn register_my_nodes() {
    register_node(
        NodeMetadata {
            id: "mySource".into(),
            display_name: "My Custom Source".into(),
            role: NodeRole::Source,
            config_aliases: vec!["MySource".into()],
            inputs: vec![],
            outputs: vec!["Imu".into()],
            default_settings: json!({}),
            settings_schema: vec![],
            subtypes: None,
            required_feature: None,
            supports_realtime_config: false,
            color: "#4ade80".into(),
        },
        |type_name, config| {
            Ok(Arc::new(Mutex::new(MySource::new(type_name, config))))
        },
    );
}
```

Then in your binary's `main()`:

```rust
fusion::registration::register_core_nodes();
my_crate::register_my_nodes();  // Your custom nodes
```

The web UI automatically discovers all registered nodes — they appear in the node editor palette, dashboard tiles, and API responses.

## UI Extension System

Third-party UI pages can be added without modifying the core frontend.

### Backend

Register an extension with metadata and an embedded JS bundle:

```rust
use fusion_registry::{register_ui_extension, UiExtensionMetadata};

register_ui_extension(
    UiExtensionMetadata {
        id: "my-view".into(),
        display_name: "My View".into(),
        route: "/my-view".into(),
        nav_section: "Views".into(),
        required_nodes: vec![],
    },
    include_bytes!("../my-view/dist/my-view.iife.js"),
);
```

### Frontend

Extensions are IIFE bundles that call `window.__FUSIONHUB__.registerPage(id, Component)`:

```typescript
const { React, useAppStore, api, registerPage } = window.__FUSIONHUB__;

function MyView() {
    const status = useAppStore((s) => s.status);
    return React.createElement('div', null, 'Hello from my extension');
}

registerPage('my-view', MyView);
```

Available globals on `window.__FUSIONHUB__`:

| Global | Description |
|--------|-------------|
| `React` | React library (classic JSX transform) |
| `ReactDOM` | ReactDOM |
| `useAppStore` | Zustand store with all app state |
| `api` | `{ apiGet, apiPost, apiPostFormData }` |
| `registerPage` | `(id, Component) => void` |

Build extensions as Vite IIFE libraries. The sidebar dynamically groups extensions by `navSection`.

## Building

### Prerequisites

- **Rust** 1.70+ (2021 edition)
- **CMake** 3.x (for building OpenZen C++ SDK)
- **Visual Studio 2022** Build Tools with C++ workload (Windows) or GCC/Clang (Linux)
- **Git** (OpenZen is cloned during build)
- **Node.js** 18+ (for building the web UI frontend)

### Build Steps

```bash
git clone --recurse-submodules <repo-url>
cd fusionhub-core

# Build the web UI frontend
cd crates/web_ui/frontend
npm install && npm run build
cd ../../..

# Build (release mode recommended for real-time performance)
cargo build --release
```

The build process automatically:
1. Compiles Protocol Buffer definitions via `prost`
2. Clones and builds the OpenZen C++ SDK via CMake
3. Copies required DLLs to the target directory (Windows)
4. Embeds the frontend dist into the binary via `include_dir`

## Usage

```bash
fusionhub-core --config config.json

# Override the WebSocket port
fusionhub-core --config config.json --port 19360
```

Once running:
- **WebSocket API** on port `19358` (default)
- **Web UI** at `http://localhost:19359`

## Configuration

FusionHub is configured through JSON files. Comments (`//` and `/* */`) are supported.

```jsonc
{
    "settings": {
        "websocketPort": 19358,
        "webUiPort": 19359
    },
    "sources": {
        "imu": [{
            "type": "OpenZen",
            "outEndpoint": "inproc://imu_data_source_0",
            "settings": { "autodetectType": "ig1" }
        }]
    },
    "sinks": {
        "echo": {},
        "websocket": {}
    }
}
```

- **Sources** produce data and publish to `outEndpoint`
- **Filters** (listed under `"sinks"`) subscribe to `inputEndpoints`, process, and publish to `dataEndpoint`
- **Sinks** consume data from accumulated upstream endpoints
- Prefix a key with `_` to disable a node

## Web UI

The embedded React web interface provides:

- **Dashboard** — Real-time node status tiles with I/O counters and rates, pause/resume and reset controls
- **Node Editor** — Visual drag-and-drop node graph editor with palette, properties panel, connection validation, config load/save
- **License** — License status, activation, and machine management
- **Extension Pages** — Dynamically loaded from registered UI extensions

## REST API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/config` | GET | Get current config |
| `/api/config` | POST | Update in-memory config |
| `/api/config/save` | POST | Save config to disk |
| `/api/config/load` | POST | Load config from a file path |
| `/api/config/save-as` | POST | Save config to a new file path |
| `/api/restart` | POST | Restart node graph |
| `/api/pause` | POST | Pause data flow |
| `/api/resume` | POST | Resume data flow |
| `/api/node-types` | GET | List all registered node types |
| `/api/ui-extensions` | GET | List registered UI extensions |
| `/ui-ext/{id}.js` | GET | Serve UI extension JS bundle |
| `/api/license/status` | GET | Get license status |
| `/api/events` | GET | SSE stream for real-time updates |

## Testing

```bash
cargo test --all
cargo test -p fusion          # Core fusion tests
cargo test -p crypto          # License system tests
cargo test -p fusion_types    # Data type tests
```

## Extending FusionHub

FusionHub Core is designed to be extended. The typical pattern:

1. Create a new Rust crate that depends on `fusion_registry` and `fusion`
2. Implement the `Node` trait for your sources/filters/sinks
3. Register them via `register_node()` with full metadata
4. Optionally register UI extensions via `register_ui_extension()`
5. Create a binary that calls your registration function alongside `register_core_nodes()`

The core crate provides all the infrastructure — ZMQ wiring, web UI, WebSocket server, config management, hot-reload, license checking — so extension crates only need to implement node logic.

## License

See [LICENSE](LICENSE) for details.
