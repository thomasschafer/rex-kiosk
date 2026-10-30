//! rex-kiosk's own config sections, read beside kiosk's shared ones.

use anyhow::Result;
use kiosk_core::config::{ConfigExtension, ManagedWorktreesConfig, OnOpenConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// rex-kiosk reads `$XDG_CONFIG_HOME/rex-kiosk/config.toml`, which is
/// `~/.config/rex-kiosk/config.toml` when `XDG_CONFIG_HOME` is unset. All
/// sections are optional, and the defaults are shown below alongside curated
/// examples for settings that benefit from one.
pub struct Config {
    /// Where kiosk creates checkouts for branches that do not have one.
    pub worktrees: ManagedWorktreesConfig,
    /// Configure command panes created in each new checkout window.
    pub on_open: OnOpenConfig,
}

impl ConfigExtension for Config {
    fn validate(&self) -> Result<()> {
        self.worktrees.validate()?;
        self.on_open.validate()
    }
}

/// The source of this module, for generating the README's config reference.
pub const SOURCE: &str = include_str!("config.rs");

#[cfg(test)]
mod tests {
    use kiosk_core::config::{
        on_open::{OnOpenPaneConfig, OnOpenPaneDirection},
        parse_config,
    };

    use super::*;

    fn parse(contents: &str) -> Result<Config> {
        parse_config::<Config>(contents).map(|(_, config, _)| config)
    }

    #[test]
    fn parses_on_open_panes_and_the_worktree_directory() {
        let (_, config, warnings) = parse_config::<Config>(
            r#"
[worktrees]
directory = "~/.herdr/worktrees"

[on_open]
panes = [
    { command = "hx", direction = "right" },
    { command = "cargo test", direction = "down", ratio = 0.35 },
]
"#,
        )
        .unwrap();

        assert_eq!(warnings, vec![]);
        assert_eq!(config.worktrees.directory, "~/.herdr/worktrees");
        assert_eq!(
            config.on_open.panes,
            [
                OnOpenPaneConfig {
                    command: "hx".into(),
                    direction: OnOpenPaneDirection::Right,
                    ratio: None,
                },
                OnOpenPaneConfig {
                    command: "cargo test".into(),
                    direction: OnOpenPaneDirection::Down,
                    ratio: Some(0.35),
                },
            ]
        );
    }

    #[test]
    fn defaults_have_no_panes_and_the_shared_worktree_directory() {
        let config = parse("").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(
            config.worktrees.directory,
            kiosk_core::config::worktrees::DEFAULT_DIRECTORY
        );
        assert_eq!(parse("[on_open]").unwrap().on_open.panes, vec![]);
    }

    #[test]
    fn rejects_invalid_on_open_panes() {
        let direction = parse(
            r#"[on_open]
panes = [{ command = "hx", direction = "left" }]"#,
        )
        .unwrap_err();
        let message = format!("{direction:#}");
        assert!(message.contains("unknown variant `left`"), "{message}");
        assert!(message.contains("expected `right` or `down`"), "{message}");

        let command = parse(
            r#"[on_open]
panes = [{ command = "  ", direction = "right" }]"#,
        )
        .unwrap_err();
        assert!(format!("{command:#}").contains("command must not be empty"));

        for ratio in ["0", "1", "-0.1", "1.1", "nan"] {
            let error = parse(&format!(
                "[on_open]\npanes = [{{ command = \"hx\", direction = \"right\", ratio = {ratio} }}]"
            ))
            .unwrap_err();
            assert!(
                format!("{error:#}").contains("ratio must be greater than 0 and less than 1"),
                "unexpected error for ratio {ratio}: {error:#}"
            );
        }
    }

    #[test]
    fn an_empty_worktree_directory_is_rejected() {
        let error = parse("[worktrees]\ndirectory = \" \"\n").unwrap_err();
        assert!(
            format!("{error:#}").contains("worktrees directory must not be empty"),
            "{error:#}"
        );
    }

    #[test]
    fn unknown_keys_in_core_and_own_sections_are_reported() {
        let (_, _, warnings) = parse_config::<Config>(
            "future = 1\n[theme]\nfuture_theme = 1\n[on_open]\nfuture_on_open = 1\n\
             [worktrees]\nfuture_worktrees = 1\n",
        )
        .unwrap();
        let messages: Vec<_> = warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert_eq!(
            messages,
            [
                "unknown config key ignored: future",
                "unknown config key ignored: theme.future_theme",
                "unknown config key ignored: on_open.future_on_open",
                "unknown config key ignored: worktrees.future_worktrees",
            ]
        );
    }
}
