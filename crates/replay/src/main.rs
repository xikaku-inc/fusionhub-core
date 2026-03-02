use clap::Parser;

/// FusionHub Replay -- replays recorded NDJSON data files at configurable speed.
///
/// Example: FHReplay -r recording.ndjson -o tcp://*:9921 --replay-speed 2.0
#[derive(Parser, Debug)]
#[command(name = "FHReplay", version, about)]
struct Cli {
    /// Input file path (NDJSON format)
    #[arg(short = 'r', long = "readfile")]
    readfile: String,

    /// Output endpoint for replayed data
    #[arg(short = 'o', long = "output-endpoint", default_value = "tcp://*:9921")]
    output_endpoint: String,

    /// Loop playback continuously
    #[arg(short = 'l', long = "loop")]
    do_loop: bool,

    /// Enable verbose debug output
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Replay speed factor (1.0 = realtime, 2.0 = double speed)
    #[arg(long = "replay-speed", default_value = "1.0")]
    replay_speed: f64,

    /// Maximum queue size
    #[arg(long = "queue-size", default_value = "100")]
    queue_size: usize,

    /// Pre-buffer delay in milliseconds
    #[arg(long = "buffer-size", default_value = "500")]
    buffer_size: u64,

    /// Echo replayed data to stdout
    #[arg(long = "echo-data")]
    echo_data: bool,
}

fn main() {
    log_utils::init();

    let cli = Cli::parse();

    log::info!("FHReplay starting");
    log::info!("Input: {}", cli.readfile);
    log::info!("Output: {}", cli.output_endpoint);
    log::info!("Speed: {}x", cli.replay_speed);

    if cli.echo_data {
        log::info!("Echo mode enabled -- data will be printed to log");
    }

    replay::replay_from_file(
        &cli.readfile,
        cli.replay_speed,
        cli.queue_size,
        &cli.output_endpoint,
        cli.do_loop,
        cli.verbose,
    );

    log::info!("FHReplay finished");
}
