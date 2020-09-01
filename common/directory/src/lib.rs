use clap::ArgMatches;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};

/// Names for the default directories.
pub const DEFAULT_ROOT_DIR: &str = ".lighthouse";
pub const DEFAULT_BEACON_NODE_DIR: &str = "beacon";
pub const DEFAULT_NETWORK_DIR: &str = "network";
pub const DEFAULT_VALIDATOR_DIR: &str = "validators";
pub const DEFAULT_SECRET_DIR: &str = "secrets";
pub const DEFAULT_WALLET_DIR: &str = "wallets";

/// Base directory name for unnamed testnets passed through the --testnet-dir flag
pub const CUSTOM_TESTNET_DIR: &str = "custom";

/// Get the default base directory as $HOME/DEFAULT_ROOT_DIR/DEFAULT_HARDCODED_TESTNET
///
/// For e.g. $HOME/.lighthouse/medalla
pub fn get_default_base_dir() -> PathBuf {
    let mut base_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base_dir.push(DEFAULT_ROOT_DIR);
    base_dir.push(eth2_testnet_config::DEFAULT_HARDCODED_TESTNET);
    base_dir
}

/// Gets the testnet directory name
///
/// Tries to get the name first from the "testnet" flag,
/// if not present, then checks the "testnet-dir" flag and returns a custom name
/// If neither flags are present, returns the default hardcoded network name.
pub fn get_testnet_dir(matches: &ArgMatches) -> String {
    if let Some(testnet_name) = matches.value_of("testnet") {
        testnet_name.to_string()
    } else if matches.value_of("testnet-dir").is_some() {
        CUSTOM_TESTNET_DIR.to_string()
    } else {
        eth2_testnet_config::DEFAULT_HARDCODED_TESTNET.to_string()
    }
}

/// Checks if a directory exists in the given path and creates a directory if it does not exist.
pub fn ensure_dir_exists<P: AsRef<Path>>(path: P) -> Result<(), String> {
    let path = path.as_ref();

    if !path.exists() {
        create_dir_all(path).map_err(|e| format!("Unable to create {:?}: {:?}", path, e))?;
    }

    Ok(())
}

/// If `name` is in `matches`, parses the value as a path. Otherwise, attempts to find the user's
/// home directory and appends the default path for the chosen testnet + the given `default_arg`.
pub fn custom_base_dir(
    matches: &ArgMatches,
    arg: &'static str,
    default_arg: &str,
) -> Result<PathBuf, String> {
    clap_utils::parse_path_with_default_in_home_dir(
        matches,
        arg,
        PathBuf::new()
            .join(DEFAULT_ROOT_DIR)
            .join(get_testnet_dir(matches))
            .join(default_arg),
    )
}
