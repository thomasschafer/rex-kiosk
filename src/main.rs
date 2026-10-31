use std::{io, sync::Arc};

use anyhow::{Context, Result};
use kiosk_core::{
    config,
    git::{CliGitProvider, GitProvider},
};

use rex_kiosk::{
    PRODUCT,
    config::Config,
    launch::{self, Command, Host, USAGE},
    rex::{CliRex, Rex, backend::Launch},
};

fn main() {
    let command = match launch::parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => fail(&error, false),
    };
    let options = match command {
        Command::Help => {
            println!("{USAGE}");
            return;
        }
        Command::Version => {
            println!("{} {}", PRODUCT.name, env!("CARGO_PKG_VERSION"));
            return;
        }
        Command::Run(options) => options,
    };
    // A launcher that passes the client runs kiosk in a pane that closes when it exits,
    // so an error waits to be read.
    let in_launched_pane = options.client.is_some();
    if let Err(error) = start(options) {
        fail(&error, in_launched_pane);
    }
}

fn fail(error: &anyhow::Error, wait: bool) -> ! {
    eprintln!("{}: {error:#}", PRODUCT.name);
    if wait {
        eprintln!("\nPress Enter to close.");
        let _ = io::stdin().read_line(&mut String::new());
    }
    std::process::exit(1);
}

fn start(options: launch::Options) -> Result<()> {
    let host = launch::detect_host(options)?;
    let loaded = config::load_config::<Config>(&PRODUCT)?;
    let git = Arc::new(CliGitProvider) as Arc<dyn GitProvider>;
    let host = match host {
        Host::Standalone => {
            let cwd = std::env::current_dir().ok();
            kiosk_tui::launch::run::<Rex, _>(&PRODUCT, loaded, git, None, cwd.as_deref())?;
            return Ok(());
        }
        Host::Rex(host) => host,
    };
    let placement = loaded
        .extension
        .worktrees
        .placement(kiosk_core::path::home_dir().as_deref())
        .context("in [worktrees] directory")?;
    let backend = Rex::new(
        Arc::new(CliRex::new(&host.rex_binary)),
        Arc::clone(&git),
        placement,
        loaded.extension.on_open.clone(),
        Launch {
            client: host.client,
            picker_block: Some(host.own_block),
        },
        Box::new(io::stdout()),
        Box::new(rex_kiosk::rex::backend::forget_saved_removal),
    );
    let cwd = match &host.origin_block {
        Some(block) => match backend.block_folder(&host.session, block) {
            Ok(folder) => folder,
            // The pane kiosk was opened from isn't a terminal, so it has no folder.
            Err(error) if matches!(error.code(), Some("conflict" | "method_not_found")) => None,
            Err(error) => {
                return Err(error).with_context(|| format!("could not find the folder of {block}"));
            }
        },
        None => std::env::current_dir().ok(),
    };
    kiosk_tui::launch::run(
        &PRODUCT,
        loaded,
        git,
        Some(Arc::new(backend)),
        cwd.as_deref(),
    )?;
    Ok(())
}
