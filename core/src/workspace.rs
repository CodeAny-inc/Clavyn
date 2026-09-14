use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Divider positions a split may hold. The renderer keeps drags inside this
/// band so neither half collapses to nothing; the same bounds are enforced
/// here because layouts also arrive straight from the frontend IPC surface.
pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;

/// A workspace is a collection of saved terminal layouts (tabs + panes),
/// associated hosts, and metadata. Users can switch between workspaces to
/// restore different working contexts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub tabs: Vec<TabLayout>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub icon: Option<String>,
    /// Human-readable description of what this workspace is for.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,
    /// Color label for visual identification (e.g. "#3b82f6").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<String>,
    /// Hosts associated with this workspace (quick-access list).
    #[serde(default)]
    pub host_ids: Vec<Uuid>,
    /// If true, restoring the workspace auto-connects all SSH panes.
    #[serde(default)]
    pub auto_connect: bool,
}

/// A tab contains one or more panes arranged in a split layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabLayout {
    pub id: Uuid,
    pub title: String,
    /// The split tree. A leaf pane has a host_id; a split has direction + children.
    pub layout: PaneLayout,
}

/// Recursive pane layout: either a leaf (single terminal) or a split (two halves).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PaneLayout {
    /// A single terminal pane connected to a host (or local terminal).
    Pane {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        host_id: Option<Uuid>,
        /// "ssh" or "local" — defaults to "ssh" if host_id is set, "local" otherwise.
        #[serde(default = "default_terminal_type")]
        terminal_type: String,
    },
    /// A split: two sub-layouts divided either horizontally or vertically.
    Split {
        direction: SplitDirection,
        /// 0.0–1.0, the position of the divider.
        #[serde(default = "default_ratio")]
        ratio: f32,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

fn default_terminal_type() -> String {
    "ssh".to_string()
}

fn default_ratio() -> f32 {
    0.5
}

impl Workspace {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            tabs: Vec::new(),
            icon: None,
            description: None,
            color: None,
            host_ids: Vec::new(),
            auto_connect: false,
        }
    }

    /// Bring an untrusted layout into the range the renderer maintains.
    ///
    /// A finite ratio outside the band is snapped, because a stale or
    /// rounded-off value is not worth failing a save over. A non-finite one is
    /// rejected instead: `serde_json` turns `f32::INFINITY` and `NaN` into JSON
    /// `null`, and `#[serde(default)]` only covers a *missing* field, so a
    /// stored `null` makes the whole store file unloadable on the next start.
    pub fn sanitize(&mut self) -> Result<()> {
        for tab in &mut self.tabs {
            tab.layout.sanitize()?;
        }
        Ok(())
    }
}

impl TabLayout {
    pub fn new(title: impl Into<String>, layout: PaneLayout) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            layout,
        }
    }
}

impl PaneLayout {
    pub fn pane(host_id: Option<Uuid>) -> Self {
        let terminal_type = if host_id.is_some() { "ssh" } else { "local" }.to_string();
        PaneLayout::Pane {
            host_id,
            terminal_type,
        }
    }

    pub fn split(
        direction: SplitDirection,
        first: PaneLayout,
        second: PaneLayout,
    ) -> Self {
        PaneLayout::Split {
            direction,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// Validate every split in the tree. See `Workspace::sanitize`.
    pub fn sanitize(&mut self) -> Result<()> {
        match self {
            PaneLayout::Pane { .. } => Ok(()),
            PaneLayout::Split {
                ratio,
                first,
                second,
                ..
            } => {
                if !ratio.is_finite() {
                    return Err(CoreError::InvalidInput(format!(
                        "split ratio must be a finite number between {MIN_RATIO} and {MAX_RATIO}, got {ratio}"
                    )));
                }
                *ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
                first.sanitize()?;
                second.sanitize()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PaneLayout, SplitDirection, TabLayout, Workspace, MAX_RATIO, MIN_RATIO};

    /// Nest `depth` splits, putting `ratio` on the innermost one so a caller
    /// can place a bad value at the bottom of a tree.
    fn split_with(ratio: f32, depth: usize) -> PaneLayout {
        let mut node = PaneLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio,
            first: Box::new(PaneLayout::pane(None)),
            second: Box::new(PaneLayout::pane(None)),
        };
        for _ in 1..depth {
            node = PaneLayout::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(PaneLayout::pane(None)),
                second: Box::new(node),
            };
        }
        node
    }

    fn workspace_with(layout: PaneLayout) -> Workspace {
        let mut ws = Workspace::new("test");
        ws.tabs.push(TabLayout::new("tab", layout));
        ws
    }

    fn innermost_ratio(layout: &PaneLayout) -> f32 {
        match layout {
            PaneLayout::Split { ratio, second, .. } => match second.as_ref() {
                nested @ PaneLayout::Split { .. } => innermost_ratio(nested),
                PaneLayout::Pane { .. } => *ratio,
            },
            PaneLayout::Pane { .. } => unreachable!("expected a split"),
        }
    }

    #[test]
    fn a_non_finite_ratio_is_rejected() {
        for bad in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let mut ws = workspace_with(split_with(bad, 1));
            let error = ws
                .sanitize()
                .expect_err("a non-finite ratio must not be accepted");
            assert!(
                error.to_string().contains("finite"),
                "unexpected error for {bad}: {error}"
            );
        }
    }

    #[test]
    fn a_non_finite_ratio_deep_inside_the_tree_is_rejected() {
        let mut ws = workspace_with(split_with(f32::INFINITY, 40));
        assert!(
            ws.sanitize().is_err(),
            "a bad ratio 40 levels down must not be accepted"
        );
    }

    #[test]
    fn a_finite_ratio_is_clamped_to_the_renderer_range() {
        for (input, expected) in [(-3.0_f32, MIN_RATIO), (0.0, MIN_RATIO), (7.5, MAX_RATIO)] {
            let mut ws = workspace_with(split_with(input, 1));
            ws.sanitize()
                .expect("a finite ratio is snapped, not refused");
            assert_eq!(innermost_ratio(&ws.tabs[0].layout), expected);
        }
    }

    #[test]
    fn an_in_range_ratio_is_left_alone() {
        let mut ws = workspace_with(split_with(0.42, 3));
        ws.sanitize().expect("sanitize");
        assert_eq!(innermost_ratio(&ws.tabs[0].layout), 0.42);
    }

    // `serde_json` rejects `1e400` as out of range but silently widens `1e39`
    // to `f32::INFINITY`, which it then writes back out as `null`.
    #[test]
    fn an_overflowing_json_literal_is_rejected_before_it_can_be_serialized() {
        let hostile = r#"{
            "id": "22222222-2222-2222-2222-222222222222",
            "name": "hostile",
            "tabs": [{
                "id": "33333333-3333-3333-3333-333333333333",
                "title": "tab",
                "layout": {
                    "type": "split",
                    "direction": "horizontal",
                    "ratio": 1e39,
                    "first": { "type": "pane", "terminal_type": "local" },
                    "second": { "type": "pane", "terminal_type": "local" }
                }
            }]
        }"#;

        let mut ws: Workspace = serde_json::from_str(hostile).expect("1e39 parses as a float");
        assert!(innermost_ratio(&ws.tabs[0].layout).is_infinite());
        assert!(serde_json::to_string(&ws)
            .expect("serialize")
            .contains("\"ratio\":null"));
        assert!(ws.sanitize().is_err());
    }
}
