use clap::Parser;

/// FusionHub Recorder -- records protobuf data from endpoints to an NDJSON file.
///
/// Example invocation:
///   FHRecorder -f out.ndjson tcp://localhost:4782
#[derive(Parser, Debug)]
#[command(name = "FHRecorder", version, about)]
struct Cli {
    /// Output file path (NDJSON format)
    #[arg(short = 'f', long = "file")]
    file: String,

    /// List of input endpoints to subscribe to
    #[arg(required = true)]
    endpoints: Vec<String>,
}

fn main() {
    log_utils::init();

    let cli = Cli::parse();

    let mut recorder = recorder::Recorder::new(&cli.file, cli.endpoints);

    // Install Ctrl+C handler
    let interrupted = recorder.interrupt_flag();
    ctrlc_handler(interrupted.clone());

    log::info!("Starting, press Ctrl+C to quit");

    if let Err(e) = recorder.run() {
        log::error!("Recorder error: {}", e);
        std::process::exit(1);
    }

    log::info!("Leaving recorder application..");
}

/// Register a Ctrl+C (SIGINT) handler that sets the given flag.
fn ctrlc_handler(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let f = flag.clone();
    ctrlc::set_handler(move || {
        log::info!("Ctrl+C received, shutting down...");
        f.store(true, std::sync::atomic::Ordering::Relaxed);
    })
    .expect("Error setting Ctrl-C handler");
}
