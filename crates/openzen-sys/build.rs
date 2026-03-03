use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let source_dir = manifest_dir.join("openzen");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let build_dir = out_dir.join("build");
    let install_dir = out_dir.clone();

    assert!(
        source_dir.join("CMakeLists.txt").exists(),
        "OpenZen submodule not initialised. Run: git submodule update --init --recursive"
    );

    build_openzen(&source_dir, &build_dir, &install_dir);

    println!(
        "cargo:rustc-link-search=native={}",
        install_dir.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=OpenZen");

    copy_dll_to_target(&install_dir);

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=openzen/src");
}

fn build_openzen(source_dir: &PathBuf, build_dir: &PathBuf, install_dir: &PathBuf) {
    std::fs::create_dir_all(build_dir).expect("Failed to create build directory");

    let status = Command::new("cmake")
        .arg(source_dir)
        .arg(format!("-B{}", build_dir.display()))
        .args(["-G", "Visual Studio 17 2022"])
        .args(["-A", "x64"])
        .arg(format!("-DCMAKE_INSTALL_PREFIX={}", install_dir.display()))
        .arg("-DZEN_BLUETOOTH=OFF")
        .arg("-DZEN_BLUETOOTH_BLE=OFF")
        .arg("-DZEN_USE_STATIC_LIBS=OFF")
        .arg("-DZEN_CSHARP=OFF")
        .arg("-DZEN_EXAMPLES=OFF")
        .arg("-DZEN_TESTS=OFF")
        .arg("-DZEN_PYTHON=OFF")
        .arg("-DZEN_NETWORK=OFF")
        .arg("-DSPDLOG_FMT_EXTERNAL=OFF")
        .arg("-DSPDLOG_FMT_EXTERNAL_HO=OFF")
        .arg("-DSPDLOG_INSTALL=OFF")
        .status()
        .expect("Failed to run cmake configure — is cmake installed?");
    assert!(status.success(), "cmake configure failed");

    let status = Command::new("cmake")
        .args(["--build", &build_dir.display().to_string()])
        .args(["--config", "RelWithDebInfo"])
        .args(["--parallel", &num_cpus().to_string()])
        .status()
        .expect("Failed to run cmake --build");
    assert!(status.success(), "cmake build failed");

    let status = Command::new("cmake")
        .args(["--install", &build_dir.display().to_string()])
        .args(["--config", "RelWithDebInfo"])
        .status()
        .expect("Failed to run cmake --install");
    assert!(status.success(), "cmake install failed");
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn copy_dll_to_target(cmake_install_dir: &PathBuf) {
    if cfg!(windows) {
        let bin_dir = cmake_install_dir.join("bin");
        if !bin_dir.exists() {
            return;
        }

        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let target_dir = match out_dir
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            Some(d) => d.to_path_buf(),
            None => return,
        };

        let dlls = ["OpenZen.dll", "SiUSBXp.dll", "PCANBasic.dll", "ftd2xx.dll"];
        for dll_name in &dlls {
            let dll_src = bin_dir.join(dll_name);
            if dll_src.exists() {
                let dll_dst = target_dir.join(dll_name);
                match std::fs::copy(&dll_src, &dll_dst) {
                    Ok(_) => println!(
                        "cargo:warning=Copied {} -> {}",
                        dll_src.display(),
                        dll_dst.display()
                    ),
                    Err(e) => println!(
                        "cargo:warning=Could not copy {} ({}). Stop any running fusionhub process and rebuild.",
                        dll_name, e
                    ),
                }
            }
        }
    }
}
