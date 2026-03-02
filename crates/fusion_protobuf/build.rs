fn main() {
    // Use locally downloaded protoc if PROTOC env var is not set.
    if std::env::var("PROTOC").is_err() {
        let manifest_dir =
            std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        // tools/protoc/bin/protoc.exe lives at the workspace root's parent
        let protoc_path = manifest_dir
            .ancestors()
            .find_map(|dir| {
                let candidate = dir.join("tools").join("protoc").join("bin").join("protoc.exe");
                candidate.exists().then_some(candidate)
            });
        if let Some(path) = protoc_path {
            std::env::set_var("PROTOC", &path);
        }
    }

    prost_build::compile_protos(&["proto/stream_data.proto"], &["proto/"])
        .expect("Failed to compile proto files");
}
