use std::{env, path::Path};

use anyhow::{Result, bail};
use kiosk_docgen::ProductDocs;
use rex_kiosk::config::{Config, SOURCE};

const DOCS: ProductDocs<'static> = ProductDocs {
    sources: &[SOURCE],
    extension: "Config",
    order: &[
        "search_dirs",
        "include_non_git",
        "worktrees",
        "theme",
        "on_open",
        "keys",
    ],
    examples: &[
        ("worktrees", "[worktrees]\ndirectory = \"~/Code/worktrees\""),
        (
            "on_open",
            "[on_open]\npanes = [\n  { command = \"hx\", direction = \"right\", ratio = 0.6 },\n]",
        ),
    ],
};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let Some(task) = args.next() else {
        bail!("usage: cargo xtask readme [--check]");
    };
    if task != "readme" {
        bail!("unknown xtask '{task}'; expected 'readme'");
    }

    let mut check = false;
    for argument in args {
        if argument == "--check" && !check {
            check = true;
        } else {
            bail!("unexpected argument '{argument}'; usage: cargo xtask readme [--check]");
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace parent");
    kiosk_docgen::generate::<Config>(&root.join("README.md"), &DOCS, check)
}
