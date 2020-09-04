use clap::ArgMatches;
use clap_utils::{parse_optional, parse_required};
use directory::{get_testnet_name, DEFAULT_ROOT_DIR, DEFAULT_SECRET_DIR, DEFAULT_VALIDATOR_DIR};
use serde_derive::{Deserialize, Serialize};
use std::path::PathBuf;
use types::{Graffiti, GRAFFITI_BYTES_LEN};

pub const DEFAULT_HTTP_SERVER: &str = "http://localhost:5052/";
/// Path to the slashing protection database within the datadir.
pub const SLASHING_PROTECTION_FILENAME: &str = "slashing_protection.sqlite";

/// Stores the core configuration for this validator instance.
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// The data directory, which stores all validator databases
    pub validator_dir: PathBuf,
    /// The directory containing the passwords to unlock validator keystores.
    pub secrets_dir: PathBuf,
    /// The http endpoint of the beacon node API.
    ///
    /// Should be similar to `http://localhost:8080`
    pub http_server: String,
    /// If true, the validator client will still poll for duties and produce blocks even if the
    /// beacon node is not synced at startup.
    pub allow_unsynced_beacon_node: bool,
    /// If true, refuse to unlock a keypair that is guarded by a lockfile.
    pub strict_lockfiles: bool,
    /// If true, don't scan the validators dir for new keystores.
    pub disable_auto_discover: bool,
    /// Graffiti to be inserted everytime we create a block.
    pub graffiti: Option<Graffiti>,
}

impl Default for Config {
    /// Build a new configuration from defaults.
    fn default() -> Self {
        let validator_dir = directory::get_default_base_dir().join(DEFAULT_VALIDATOR_DIR);
        let secrets_dir = directory::get_default_base_dir().join(DEFAULT_SECRET_DIR);
        Self {
            validator_dir,
            secrets_dir,
            http_server: DEFAULT_HTTP_SERVER.to_string(),
            allow_unsynced_beacon_node: false,
            strict_lockfiles: false,
            disable_auto_discover: false,
            graffiti: None,
        }
    }
}

impl Config {
    /// Returns a `Default` implementation of `Self` with some parameters modified by the supplied
    /// `cli_args`.
    pub fn from_cli(cli_args: &ArgMatches) -> Result<Config, String> {
        let mut config = Config::default();

        let default_root_dir = dirs::home_dir()
            .map(|home| home.join(DEFAULT_ROOT_DIR))
            .unwrap_or_else(|| PathBuf::from("."));

        let (mut validator_dir, mut secrets_dir) = (None, None);
        if cli_args.value_of("datadir").is_some() {
            let base_dir: PathBuf = parse_required(cli_args, "datadir")?;
            validator_dir = Some(base_dir.join(DEFAULT_VALIDATOR_DIR));
            secrets_dir = Some(base_dir.join(DEFAULT_SECRET_DIR));
        }
        if cli_args.value_of("validators-dir").is_some()
            && cli_args.value_of("secrets-dir").is_some()
        {
            validator_dir = Some(parse_required(cli_args, "validators-dir")?);
            secrets_dir = Some(parse_required(cli_args, "secrets-dir")?);
        }

        config.validator_dir = validator_dir.unwrap_or_else(|| {
            default_root_dir
                .join(get_testnet_name(cli_args))
                .join(DEFAULT_VALIDATOR_DIR)
        });

        config.secrets_dir = secrets_dir.unwrap_or_else(|| {
            default_root_dir
                .join(get_testnet_name(cli_args))
                .join(DEFAULT_SECRET_DIR)
        });

        if !config.validator_dir.exists() {
            return Err(format!(
                "The directory for validator data does not exist: {:?}",
                config.validator_dir
            ));
        }

        if let Some(server) = parse_optional(cli_args, "server")? {
            config.http_server = server;
        }

        config.allow_unsynced_beacon_node = cli_args.is_present("allow-unsynced");
        config.strict_lockfiles = cli_args.is_present("strict-lockfiles");
        config.disable_auto_discover = cli_args.is_present("disable-auto-discover");

        if let Some(input_graffiti) = cli_args.value_of("graffiti") {
            let graffiti_bytes = input_graffiti.as_bytes();
            if graffiti_bytes.len() > GRAFFITI_BYTES_LEN {
                return Err(format!(
                    "Your graffiti is too long! {} bytes maximum!",
                    GRAFFITI_BYTES_LEN
                ));
            } else {
                // Default graffiti to all 0 bytes.
                let mut graffiti = Graffiti::default();

                // Copy the provided bytes over.
                //
                // Panic-free because `graffiti_bytes.len()` <= `GRAFFITI_BYTES_LEN`.
                graffiti[..graffiti_bytes.len()].copy_from_slice(&graffiti_bytes);

                config.graffiti = Some(graffiti);
            }
        }

        Ok(config)
    }
}
