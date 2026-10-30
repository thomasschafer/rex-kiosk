//! The window layouts kiosk asks Rex to create, in the form `session.create` and
//! `session.new_window` take.

use serde::Serialize;

use super::SHELL_FLAVOR;
use kiosk_core::config::{OnOpenConfig, on_open::OnOpenPaneDirection};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Layout {
    Block(Block),
    Split(Box<Split>),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Block {
    flavor: &'static str,
    options: TerminalOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct TerminalOptions {
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    initial_input: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Split {
    direction: SplitDirection,
    before: Layout,
    after: Layout,
    /// The share of the space `before` gets. Rex halves the space when it is left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    ratio: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SplitDirection {
    /// `before` on the left of `after`.
    Horizontal,
    /// `before` above `after`.
    Vertical,
}

impl Layout {
    /// An interactive shell starting in `cwd`, with `input` typed into it once it starts.
    fn shell(cwd: &str, input: Option<String>) -> Self {
        Self::Block(Block {
            flavor: SHELL_FLAVOR,
            options: TerminalOptions {
                cwd: cwd.into(),
                initial_input: input,
            },
        })
    }

    /// A window with one shell in `cwd`.
    pub(crate) fn folder(cwd: &str) -> Self {
        Self::shell(cwd, None)
    }

    /// A window with a shell in `cwd`, beside which each `[on_open]` pane splits off with
    /// its command typed into its own shell. The first shell stays first in layout order,
    /// which is where Rex puts focus.
    pub(crate) fn checkout(cwd: &str, on_open: &OnOpenConfig) -> Self {
        on_open
            .panes
            .iter()
            .fold(Self::shell(cwd, None), |layout, pane| {
                let mut command = pane.command.clone();
                command.push('\n');
                layout.split_first(
                    match pane.direction {
                        OnOpenPaneDirection::Right => SplitDirection::Horizontal,
                        OnOpenPaneDirection::Down => SplitDirection::Vertical,
                    },
                    Self::shell(cwd, Some(command)),
                    // The config gives the new pane's share; Rex takes the first pane's.
                    pane.ratio.map(|ratio| 1.0 - ratio),
                )
            })
    }

    /// Splits the first block, putting `after` beside it.
    fn split_first(self, direction: SplitDirection, after: Self, ratio: Option<f64>) -> Self {
        match self {
            Self::Block(_) => Self::Split(Box::new(Split {
                direction,
                before: self,
                after,
                ratio,
            })),
            Self::Split(split) => {
                let Split {
                    direction: outer,
                    before,
                    after: rest,
                    ratio: outer_ratio,
                } = *split;
                Self::Split(Box::new(Split {
                    direction: outer,
                    before: before.split_first(direction, after, ratio),
                    after: rest,
                    ratio: outer_ratio,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use kiosk_core::config::on_open::OnOpenPaneConfig;

    use super::*;

    fn pane(command: &str, direction: OnOpenPaneDirection, ratio: Option<f64>) -> OnOpenPaneConfig {
        OnOpenPaneConfig {
            command: command.into(),
            direction,
            ratio,
        }
    }

    fn shell(input: Option<&str>) -> serde_json::Value {
        let mut options = json!({ "cwd": "/repo" });
        if let Some(input) = input {
            options["initial_input"] = input.into();
        }
        json!({ "block": { "flavor": SHELL_FLAVOR, "options": options } })
    }

    #[test]
    fn without_panes_a_checkout_is_one_shell() {
        let layout = Layout::checkout("/repo", &OnOpenConfig::default());
        assert_eq!(serde_json::to_value(&layout).unwrap(), shell(None));
        assert_eq!(layout, Layout::folder("/repo"));
    }

    #[test]
    fn each_pane_splits_the_first_shell_with_its_share_and_command() {
        let on_open = OnOpenConfig {
            panes: vec![
                pane("hx", OnOpenPaneDirection::Right, Some(0.25)),
                pane("cargo test", OnOpenPaneDirection::Down, None),
            ],
        };

        let layout = serde_json::to_value(Layout::checkout("/repo", &on_open)).unwrap();

        assert_eq!(
            layout,
            json!({ "split": {
                "direction": "horizontal",
                "ratio": 0.75,
                "before": { "split": {
                    "direction": "vertical",
                    "before": shell(None),
                    "after": shell(Some("cargo test\n")),
                } },
                "after": shell(Some("hx\n")),
            } })
        );
    }
}
