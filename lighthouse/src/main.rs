#[macro_use]
extern crate clap;

use beacon_node;
use clap::{App, Arg, ArgMatches};
use env_logger::{Builder, Env};
use environment::EnvironmentBuilder;
use slog::{crit, info, warn};
use std::process::exit;
use types::{EthSpec, InteropEthSpec, MainnetEthSpec, MinimalEthSpec};

pub const DEFAULT_DATA_DIR: &str = ".lighthouse";
pub const CLIENT_CONFIG_FILENAME: &str = "beacon-node.toml";
pub const ETH2_CONFIG_FILENAME: &str = "eth2-spec.toml";
pub const TESTNET_CONFIG_FILENAME: &str = "testnet.toml";

fn main() {
    // Debugging output for libp2p and external crates.
    Builder::from_env(Env::default()).init();

    // Parse the CLI parameters.
    let matches = App::new("Lighthouse")
        .version(crate_version!())
        .author("Sigma Prime <contact@sigmaprime.io>")
        .about("Eth 2.0 Client")
        .arg(
            Arg::with_name("spec")
                .short("s")
                .long("spec")
                .value_name("TITLE")
                .help("Specifies the default eth2 spec type. Only effective when creating a new datadir.")
                .takes_value(true)
                .required(true)
                .possible_values(&["mainnet", "minimal", "interop"])
                .global(true)
                .default_value("minimal")
        )
        .arg(
            Arg::with_name("logfile")
                .long("logfile")
                .value_name("FILE")
                .help("File path where output will be written.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("debug-level")
                .long("debug-level")
                .value_name("LEVEL")
                .help("The title of the spec constants for chain config.")
                .takes_value(true)
                .possible_values(&["info", "debug", "trace", "warn", "error", "crit"])
                .default_value("trace"),
        )
        .subcommand(beacon_node::cli_app())
        .get_matches();

    macro_rules! run_with_spec {
        ($eth_spec: ident) => {
            match run($eth_spec, &matches) {
                Ok(()) => exit(0),
                Err(e) => {
                    println!("Failed to start Lighthouse: {}", e);
                    exit(1)
                }
            }
        };
    }

    match matches.value_of("spec") {
        Some("minimal") => run_with_spec!(MinimalEthSpec),
        Some("mainnet") => run_with_spec!(MainnetEthSpec),
        Some("interop") => run_with_spec!(InteropEthSpec),
        spec => {
            // This path should be unreachable due to slog having a `default_value`
            unreachable!("Unknown spec configuration: {:?}", spec);
        }
    }
}

fn run<E: EthSpec>(eth_spec_instance: E, matches: &ArgMatches) -> Result<(), String> {
    let mut environment = EnvironmentBuilder::new(eth_spec_instance)
        .async_logger(
            matches
                .value_of("debug-level")
                .ok_or_else(|| "Expected --debug-level flag".to_string())?,
        )?
        .tokio_runtime()?
        .build()?;

    let log = environment.core_log();

    if std::mem::size_of::<usize>() != 8 {
        crit!(
            log,
            "Lighthouse only supports 64bit CPUs";
            "detected" => format!("{}bit", std::mem::size_of::<usize>() * 8)
        );
        return Err("Invalid CPU architecture".into());
    }

    warn!(
        log,
        "Ethereum 2.0 is pre-release. This software is experimental."
    );

    let beacon_node = if let Some(sub_matches) = matches.subcommand_matches("Beacon Node") {
        Some(beacon_node::start_from_cli(sub_matches, &environment)?)
    } else {
        None
    };

    if beacon_node.is_none() {
        crit!(log, "No subcommand supplied. See --help .");
        return Err("No subcommand supplied.".into());
    }

    // Block this thread until Crtl+C is pressed.
    environment.block_until_ctrl_c()?;

    info!(log, "Shutting down..");

    // Drop the beacon node (if it was started), cleanly shutting down all related services.
    drop(beacon_node);

    // Shutdown the environment once all tasks have completed.
    environment.shutdown_on_idle()
}
