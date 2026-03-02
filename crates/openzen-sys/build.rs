use std::env;
use std::path::PathBuf;
use std::process::Command;

const OPENZEN_REPO: &str = "https://bitbucket.org/lpresearch/openzen.git";
const OPENZEN_COMMIT: &str = "dd8d9269ccbbb5a3582ac4ecb622ae829f67aee7";

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let source_dir = out_dir.join("openzen-src");
    let build_dir = out_dir.join("build");
    let install_dir = out_dir.clone();

    // Only clone if source doesn't exist yet (persists across incremental builds)
    if !source_dir.join("CMakeLists.txt").exists() {
        clone_and_prepare(&source_dir);
    }

    // Build with cmake directly — avoid the cmake crate which overrides MSVC's
    // default compiler flags (strips /O2, /DNDEBUG, /Zi from RelWithDebInfo).
    build_openzen(&source_dir, &build_dir, &install_dir);

    // Link directives — import library is in lib/, DLL is in bin/ (Windows)
    println!(
        "cargo:rustc-link-search=native={}",
        install_dir.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=OpenZen");

    // Copy DLL next to the final executable so it's found at runtime
    copy_dll_to_target(&install_dir);

    // Only re-run build script if these files change
    println!("cargo:rerun-if-changed=build.rs");
}

fn build_openzen(source_dir: &PathBuf, build_dir: &PathBuf, install_dir: &PathBuf) {
    std::fs::create_dir_all(build_dir).expect("Failed to create build directory");

    // Configure — use Visual Studio generator (reliable MSVC detection on
    // Windows) with default compiler flags.  By NOT overriding CMAKE_C_FLAGS /
    // CMAKE_CXX_FLAGS, cmake keeps its standard per-config defaults:
    //   RelWithDebInfo -> /MD /Zi /O2 /Ob1 /DNDEBUG
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

    // Build with RelWithDebInfo configuration
    let status = Command::new("cmake")
        .args(["--build", &build_dir.display().to_string()])
        .args(["--config", "RelWithDebInfo"])
        .args(["--parallel", &num_cpus().to_string()])
        .status()
        .expect("Failed to run cmake --build");
    assert!(status.success(), "cmake build failed");

    // Install
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

fn clone_and_prepare(dest: &PathBuf) {
    println!("cargo:warning=Cloning OpenZen from {} ...", OPENZEN_REPO);

    let status = Command::new("git")
        .args(["clone", "--no-checkout", OPENZEN_REPO])
        .arg(dest)
        .status()
        .expect("Failed to run `git clone` — is git installed?");
    assert!(status.success(), "git clone failed");

    let status = Command::new("git")
        .args(["checkout", OPENZEN_COMMIT])
        .current_dir(dest)
        .status()
        .expect("Failed to run `git checkout`");
    assert!(status.success(), "git checkout failed");

    // Only init the submodules we actually need (skip googletest, zmq, pybind11)
    let required_submodules = [
        "external/gsl",
        "external/expected-lite",
        "external/spdlog",
        "external/asio",
        "external/cereal",
    ];

    for sub in &required_submodules {
        println!("cargo:warning=  Initialising submodule {} ...", sub);
        let status = Command::new("git")
            .args(["submodule", "update", "--init", sub])
            .current_dir(dest)
            .status()
            .unwrap_or_else(|_| panic!("Failed to init submodule {}", sub));
        assert!(status.success(), "git submodule update --init {} failed", sub);
    }

    println!("cargo:warning=OpenZen source ready at {}", dest.display());
}

fn copy_dll_to_target(cmake_install_dir: &PathBuf) {
    // On Windows, OpenZen.dll and its companion DLLs (SiUSBXp, PCANBasic,
    // ftd2xx) must be next to the executable or on PATH.  Copy everything
    // from the cmake bin/ directory to the target profile directory.
    if cfg!(windows) {
        let bin_dir = cmake_install_dir.join("bin");
        if !bin_dir.exists() {
            return;
        }

        // OUT_DIR is like: target/debug/build/openzen-sys-<hash>/out
        // Walk up to:      target/debug/
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let target_dir = match out_dir
            .parent() // build/openzen-sys-<hash>
            .and_then(|p| p.parent()) // build/
            .and_then(|p| p.parent()) // target/debug/
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
