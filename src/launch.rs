//! How rex-kiosk was started: from its command line and the environment Rex gives every
//! terminal it runs.

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, bail};

use crate::rex::{BlockId, ClientId, SessionId};

/// Set by Rex in every terminal: the directory holding the `rex` CLI that talks to the
/// server the terminal runs on.
pub const REX_BIN_DIR: &str = "REX_BIN_DIR";
/// Set by Rex in every terminal: the session the terminal is in.
pub const REX_SESSION: &str = "REX_SESSION";
/// Set by Rex in every terminal: the terminal's own block.
pub const REX_BLOCK: &str = "REX_BLOCK";

pub const USAGE: &str = "\
usage: rex-kiosk [--client CLIENT] [--block BLOCK]

Fuzzy-find a Git repository, branch or folder and open it in Rex.

options:
  --client CLIENT  the Rex app to show what opens in (a client: ID); defaults to
                   the only app connected to the server
  --block BLOCK    the block (a block: ID) whose folder to preselect, in the same
                   session as rex-kiosk; defaults to rex-kiosk's own folder
  -h, --help       print this help
  -V, --version    print the version

Rex's kiosk action passes both options; see the README.";

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Help,
    Version,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub client: Option<ClientId>,
    pub block: Option<BlockId>,
}

pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else {
            bail!("argument {} is not valid UTF-8\n\n{USAGE}", arg.display());
        };
        match arg {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--client" => set_once(&mut options.client, "--client", args.next())?,
            "--block" => set_once(&mut options.block, "--block", args.next())?,
            other => bail!("unexpected argument {other:?}\n\n{USAGE}"),
        }
    }
    Ok(Command::Run(options))
}

fn set_once<T: TryFrom<String, Error = String>>(
    slot: &mut Option<T>,
    name: &str,
    value: Option<OsString>,
) -> Result<()> {
    if slot.is_some() {
        bail!("{name} was given twice");
    }
    let Some(value) = value.and_then(|value| value.into_string().ok()) else {
        bail!("{name} needs a value\n\n{USAGE}");
    };
    *slot = Some(T::try_from(value).map_err(|error| anyhow::anyhow!("{name}: {error}"))?);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// Not in a Rex terminal, so there is nothing to open things in.
    Standalone,
    Rex(RexHost),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RexHost {
    pub rex_binary: PathBuf,
    /// The session and block rex-kiosk runs in.
    pub session: SessionId,
    pub own_block: BlockId,
    pub client: Option<ClientId>,
    /// The block whose folder the user was in, if the launcher said.
    pub origin_block: Option<BlockId>,
}

pub fn detect_host(options: Options) -> Result<Host> {
    detect_host_with(options, |name| std::env::var_os(name))
}

pub fn detect_host_with(
    options: Options,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<Host> {
    let get = |name: &str| get_env(name).filter(|value| !value.is_empty());
    let Some(bin_dir) = get(REX_BIN_DIR) else {
        if options.client.is_some() || options.block.is_some() {
            bail!("--client and --block need a Rex terminal, but {REX_BIN_DIR} is not set");
        }
        return Ok(Host::Standalone);
    };
    let bin_dir = PathBuf::from(bin_dir);
    if !bin_dir.is_absolute() {
        bail!(
            "{REX_BIN_DIR} must be an absolute path: {}",
            bin_dir.display()
        );
    }
    let id = |name: &str| -> Result<String> {
        let Some(value) = get(name) else {
            bail!("{name} must be set when {REX_BIN_DIR} is");
        };
        value
            .into_string()
            .map_err(|value| anyhow::anyhow!("{name} is not valid UTF-8: {}", value.display()))
    };
    let session = SessionId::try_from(id(REX_SESSION)?)
        .map_err(|error| anyhow::anyhow!("{REX_SESSION}: {error}"))?;
    let own_block = BlockId::try_from(id(REX_BLOCK)?)
        .map_err(|error| anyhow::anyhow!("{REX_BLOCK}: {error}"))?;
    Ok(Host::Rex(RexHost {
        rex_binary: bin_dir.join("rex"),
        session,
        own_block,
        client: options.client,
        origin_block: options.block,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn args(args: &[&str]) -> Result<Command> {
        parse_args(args.iter().map(OsString::from))
    }

    fn detect(options: Options, vars: &[(&str, &str)]) -> Result<Host> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect();
        detect_host_with(options, |name| vars.get(name).cloned())
    }

    const REX: [(&str, &str); 3] = [
        (REX_BIN_DIR, "/Applications/Rex.app/bin"),
        (REX_SESSION, "session:01a1"),
        (REX_BLOCK, "block:01b2"),
    ];

    #[test]
    fn options_take_rex_ids() {
        assert_eq!(args(&[]).unwrap(), Command::Run(Options::default()));
        assert_eq!(
            args(&["--client", "client:1", "--block", "block:2"]).unwrap(),
            Command::Run(Options {
                client: Some(ClientId::try_from("client:1".to_string()).unwrap()),
                block: Some(BlockId::try_from("block:2".to_string()).unwrap()),
            })
        );
        assert_eq!(args(&["--version"]).unwrap(), Command::Version);
        assert_eq!(args(&["-h", "--bogus"]).unwrap(), Command::Help);
    }

    #[test]
    fn bad_options_are_explained() {
        for (given, expected) in [
            (&["--client"][..], "--client needs a value"),
            (&["--client", "block:2"], "starting with \"client:\""),
            (&["--block", "block:1", "--block", "block:2"], "given twice"),
            (&["extra"], "unexpected argument"),
        ] {
            let error = format!("{:#}", args(given).unwrap_err());
            assert!(error.contains(expected), "{given:?}: {error}");
        }
    }

    #[test]
    fn outside_rex_kiosk_is_standalone() {
        assert_eq!(detect(Options::default(), &[]).unwrap(), Host::Standalone);
        assert_eq!(
            detect(Options::default(), &[(REX_BIN_DIR, "")]).unwrap(),
            Host::Standalone
        );
        let error = detect(
            Options {
                client: Some(ClientId::try_from("client:1".to_string()).unwrap()),
                block: None,
            },
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains(REX_BIN_DIR), "{error}");
    }

    #[test]
    fn in_rex_kiosk_uses_the_terminals_rex_and_ids() {
        let options = Options {
            client: Some(ClientId::try_from("client:9".to_string()).unwrap()),
            block: Some(BlockId::try_from("block:7".to_string()).unwrap()),
        };
        let host = detect(options.clone(), &REX).unwrap();
        assert_eq!(
            host,
            Host::Rex(RexHost {
                rex_binary: "/Applications/Rex.app/bin/rex".into(),
                session: SessionId::try_from("session:01a1".to_string()).unwrap(),
                own_block: BlockId::try_from("block:01b2".to_string()).unwrap(),
                client: options.client,
                origin_block: options.block,
            })
        );
    }

    #[test]
    fn a_rex_terminal_must_say_where_it_is() {
        for missing in [REX_SESSION, REX_BLOCK] {
            let vars: Vec<_> = REX
                .into_iter()
                .filter(|(name, _)| *name != missing)
                .collect();
            let error = detect(Options::default(), &vars).unwrap_err().to_string();
            assert!(error.contains(missing), "{error}");
        }
        let mut vars = REX.to_vec();
        vars[0] = (REX_BIN_DIR, "bin");
        let error = detect(Options::default(), &vars).unwrap_err().to_string();
        assert!(error.contains("absolute"), "{error}");
        vars[0] = REX[0];
        vars[1] = (REX_SESSION, "block:1");
        let error = detect(Options::default(), &vars).unwrap_err().to_string();
        assert!(error.contains(REX_SESSION), "{error}");
    }
}
