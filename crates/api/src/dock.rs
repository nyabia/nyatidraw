use std::collections::BTreeSet;

const MIN_SPLIT_PER_MILLE: u16 = 20;
const MAX_SPLIT_PER_MILLE: u16 = 980;
const MAX_DOCK_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PanelKind {
    Canvas,
    Tools,
    Navigator,
    Layers,
    Brush,
    Color,
    History,
}

impl PanelKind {
    const REQUIRED: [Self; 7] = [
        Self::Canvas,
        Self::Tools,
        Self::Navigator,
        Self::Layers,
        Self::Brush,
        Self::Color,
        Self::History,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockAxis {
    Horizontal,
    Vertical,
}

/// Where a panel is placed relative to another panel in the workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockPosition {
    Left,
    Right,
    Top,
    Bottom,
    Tab,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockMutationError {
    SamePanel,
    TargetMissing(PanelKind),
    LayoutInvalid(DockLayoutError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DockNode {
    Panel(PanelKind),
    Tabs {
        active: usize,
        panels: Vec<PanelKind>,
    },
    Split {
        axis: DockAxis,
        /// Size of the first child in thousandths of the available extent.
        first_per_mille: u16,
        first: Box<Self>,
        second: Box<Self>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockLayoutError {
    CorruptPayload,
    TooDeep,
    EmptyTabs,
    InvalidActiveTab { active: usize, len: usize },
    InvalidSplitRatio(u16),
    DuplicatePanel(PanelKind),
    MissingPanel(PanelKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockTree {
    root: DockNode,
}

impl DockTree {
    /// Accepts a complete backend-neutral dock tree after validating all
    /// panels, split ratios, tab indices, and recursion bounds.
    ///
    /// # Errors
    ///
    /// Returns the first structural error in deterministic traversal order.
    pub fn new(root: DockNode) -> Result<Self, DockLayoutError> {
        let mut panels = BTreeSet::new();
        validate_node(&root, 0, &mut panels)?;
        for required in PanelKind::REQUIRED {
            if !panels.contains(&required) {
                return Err(DockLayoutError::MissingPanel(required));
            }
        }
        Ok(Self { root })
    }

    /// Converts either a decoded tree or a load failure into a usable layout.
    /// Invalid input is never partially retained.
    #[must_use]
    pub fn recover(decoded: Result<DockNode, DockLayoutError>) -> DockLayoutRecovery {
        match decoded.and_then(Self::new) {
            Ok(tree) => DockLayoutRecovery {
                tree,
                status: DockLayoutRecoveryStatus::Loaded,
            },
            Err(error) => DockLayoutRecovery {
                tree: Self::safe_default(),
                status: DockLayoutRecoveryStatus::SafeDefault(error),
            },
        }
    }

    #[must_use]
    pub fn safe_default() -> Self {
        let root = DockNode::Split {
            axis: DockAxis::Horizontal,
            first_per_mille: 30,
            first: Box::new(DockNode::Panel(PanelKind::Tools)),
            second: Box::new(DockNode::Split {
                axis: DockAxis::Horizontal,
                first_per_mille: 80,
                first: Box::new(DockNode::Panel(PanelKind::Brush)),
                second: Box::new(DockNode::Split {
                    axis: DockAxis::Horizontal,
                    first_per_mille: 860,
                    first: Box::new(DockNode::Panel(PanelKind::Canvas)),
                    second: Box::new(DockNode::Split {
                        axis: DockAxis::Vertical,
                        first_per_mille: 250,
                        first: Box::new(DockNode::Panel(PanelKind::Navigator)),
                        second: Box::new(DockNode::Split {
                            axis: DockAxis::Vertical,
                            first_per_mille: 330,
                            first: Box::new(DockNode::Panel(PanelKind::Color)),
                            second: Box::new(DockNode::Tabs {
                                active: 0,
                                panels: vec![PanelKind::Layers, PanelKind::History],
                            }),
                        }),
                    }),
                }),
            }),
        };
        Self { root }
    }

    #[must_use]
    pub const fn root(&self) -> &DockNode {
        &self.root
    }

    /// Makes a panel visible in its containing tab set. Standalone panels are
    /// already visible. Returns whether the tree changed.
    pub fn activate_panel(&mut self, panel: PanelKind) -> bool {
        activate_panel(&mut self.root, panel)
    }

    /// Moves one panel into a tab set or beside another panel. The result is
    /// revalidated before it becomes visible, so shell controls cannot create
    /// a layout that loses access to a required panel.
    ///
    /// # Errors
    ///
    /// Returns an error if the target is absent, is the moving panel itself,
    /// or if a future change makes the produced tree invalid.
    pub fn dock_panel(
        &mut self,
        panel: PanelKind,
        target: PanelKind,
        position: DockPosition,
    ) -> Result<(), DockMutationError> {
        if panel == target {
            return Err(DockMutationError::SamePanel);
        }
        if !contains_panel(&self.root, target) {
            return Err(DockMutationError::TargetMissing(target));
        }
        let Some(without_panel) = remove_panel(self.root.clone(), panel) else {
            return Err(DockMutationError::LayoutInvalid(
                DockLayoutError::MissingPanel(panel),
            ));
        };
        let Some(root) = insert_docked_panel(without_panel, panel, target, position) else {
            return Err(DockMutationError::TargetMissing(target));
        };
        let next = Self::new(root).map_err(DockMutationError::LayoutInvalid)?;
        self.root = next.root;
        Ok(())
    }
}

impl Default for DockTree {
    fn default() -> Self {
        Self::safe_default()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockLayoutRecoveryStatus {
    Loaded,
    SafeDefault(DockLayoutError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockLayoutRecovery {
    pub tree: DockTree,
    pub status: DockLayoutRecoveryStatus,
}

fn validate_node(
    node: &DockNode,
    depth: usize,
    panels: &mut BTreeSet<PanelKind>,
) -> Result<(), DockLayoutError> {
    if depth > MAX_DOCK_DEPTH {
        return Err(DockLayoutError::TooDeep);
    }
    match node {
        DockNode::Panel(panel) => insert_panel(*panel, panels),
        DockNode::Tabs {
            active,
            panels: tabs,
        } => {
            if tabs.is_empty() {
                return Err(DockLayoutError::EmptyTabs);
            }
            if *active >= tabs.len() {
                return Err(DockLayoutError::InvalidActiveTab {
                    active: *active,
                    len: tabs.len(),
                });
            }
            for panel in tabs {
                insert_panel(*panel, panels)?;
            }
            Ok(())
        }
        DockNode::Split {
            first_per_mille,
            first,
            second,
            ..
        } => {
            if !(MIN_SPLIT_PER_MILLE..=MAX_SPLIT_PER_MILLE).contains(first_per_mille) {
                return Err(DockLayoutError::InvalidSplitRatio(*first_per_mille));
            }
            validate_node(first, depth + 1, panels)?;
            validate_node(second, depth + 1, panels)
        }
    }
}

fn insert_panel(panel: PanelKind, panels: &mut BTreeSet<PanelKind>) -> Result<(), DockLayoutError> {
    if panels.insert(panel) {
        Ok(())
    } else {
        Err(DockLayoutError::DuplicatePanel(panel))
    }
}

fn activate_panel(node: &mut DockNode, panel: PanelKind) -> bool {
    match node {
        DockNode::Panel(_) => false,
        DockNode::Tabs { active, panels } => {
            let Some(index) = panels.iter().position(|candidate| *candidate == panel) else {
                return false;
            };
            let changed = *active != index;
            *active = index;
            changed
        }
        DockNode::Split { first, second, .. } => {
            activate_panel(first, panel) || activate_panel(second, panel)
        }
    }
}

fn remove_panel(node: DockNode, panel: PanelKind) -> Option<DockNode> {
    match node {
        DockNode::Panel(candidate) => (candidate != panel).then_some(DockNode::Panel(candidate)),
        DockNode::Tabs { active, mut panels } => {
            panels.retain(|candidate| *candidate != panel);
            match panels.len() {
                0 => None,
                1 => Some(DockNode::Panel(panels[0])),
                len => Some(DockNode::Tabs {
                    active: active.min(len - 1),
                    panels,
                }),
            }
        }
        DockNode::Split {
            axis,
            first_per_mille,
            first,
            second,
        } => match (remove_panel(*first, panel), remove_panel(*second, panel)) {
            (Some(first), Some(second)) => Some(DockNode::Split {
                axis,
                first_per_mille,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
            (None, None) => None,
        },
    }
}

fn insert_docked_panel(
    node: DockNode,
    panel: PanelKind,
    target: PanelKind,
    position: DockPosition,
) -> Option<DockNode> {
    match node {
        DockNode::Panel(candidate) if candidate == target => {
            Some(place_beside(DockNode::Panel(candidate), panel, position))
        }
        DockNode::Panel(candidate) => Some(DockNode::Panel(candidate)),
        DockNode::Tabs { active, mut panels } => {
            if panels.contains(&target) {
                if position == DockPosition::Tab {
                    panels.push(panel);
                    Some(DockNode::Tabs {
                        active: panels.len() - 1,
                        panels,
                    })
                } else {
                    Some(place_beside(
                        DockNode::Tabs { active, panels },
                        panel,
                        position,
                    ))
                }
            } else {
                Some(DockNode::Tabs { active, panels })
            }
        }
        DockNode::Split {
            axis,
            first_per_mille,
            first,
            second,
        } => {
            let first = insert_docked_panel(*first, panel, target, position)?;
            if contains_panel(&first, panel) {
                return Some(DockNode::Split {
                    axis,
                    first_per_mille,
                    first: Box::new(first),
                    second,
                });
            }
            let second = insert_docked_panel(*second, panel, target, position)?;
            if contains_panel(&second, panel) {
                return Some(DockNode::Split {
                    axis,
                    first_per_mille,
                    first: Box::new(first),
                    second: Box::new(second),
                });
            }
            Some(DockNode::Split {
                axis,
                first_per_mille,
                first: Box::new(first),
                second: Box::new(second),
            })
        }
    }
}

fn contains_panel(node: &DockNode, panel: PanelKind) -> bool {
    match node {
        DockNode::Panel(candidate) => *candidate == panel,
        DockNode::Tabs { panels, .. } => panels.contains(&panel),
        DockNode::Split { first, second, .. } => {
            contains_panel(first, panel) || contains_panel(second, panel)
        }
    }
}

fn place_beside(target: DockNode, panel: PanelKind, position: DockPosition) -> DockNode {
    if position == DockPosition::Tab {
        return DockNode::Tabs {
            active: 1,
            panels: vec![first_panel(&target), panel],
        };
    }
    let (axis, moving_first) = match position {
        DockPosition::Left => (DockAxis::Horizontal, true),
        DockPosition::Right => (DockAxis::Horizontal, false),
        DockPosition::Top => (DockAxis::Vertical, true),
        DockPosition::Bottom => (DockAxis::Vertical, false),
        DockPosition::Tab => unreachable!("handled above"),
    };
    let moving = Box::new(DockNode::Panel(panel));
    let target = Box::new(target);
    let (first, second) = if moving_first {
        (moving, target)
    } else {
        (target, moving)
    };
    DockNode::Split {
        axis,
        first_per_mille: 500,
        first,
        second,
    }
}

fn first_panel(node: &DockNode) -> PanelKind {
    match node {
        DockNode::Panel(panel) => *panel,
        DockNode::Tabs { panels, .. } => panels[0],
        DockNode::Split { first, .. } => first_panel(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_session_layout_recovers_to_the_complete_safe_default() {
        let corrupt = DockNode::Split {
            axis: DockAxis::Horizontal,
            first_per_mille: 999,
            first: Box::new(DockNode::Panel(PanelKind::Canvas)),
            second: Box::new(DockNode::Panel(PanelKind::Canvas)),
        };
        let recovery = DockTree::recover(Ok(corrupt));
        assert_eq!(
            recovery.status,
            DockLayoutRecoveryStatus::SafeDefault(DockLayoutError::InvalidSplitRatio(999)),
            "corrupt session geometry must never leak into the active layout"
        );
        assert_eq!(recovery.tree, DockTree::safe_default());

        let decode_failure = DockTree::recover(Err(DockLayoutError::CorruptPayload));
        assert_eq!(decode_failure.tree, DockTree::safe_default());
        assert_eq!(
            decode_failure.status,
            DockLayoutRecoveryStatus::SafeDefault(DockLayoutError::CorruptPayload)
        );
    }

    #[test]
    fn panel_moves_keep_every_required_panel_reachable() {
        let mut tree = DockTree::safe_default();
        tree.dock_panel(PanelKind::Color, PanelKind::Canvas, DockPosition::Left)
            .expect("directional docking must preserve the complete workspace");
        tree.dock_panel(PanelKind::History, PanelKind::Layers, DockPosition::Tab)
            .expect("tab docking must preserve the complete workspace");
        DockTree::new(tree.root.clone())
            .expect("a sequence of shell moves must not corrupt layout access");
        assert!(tree.activate_panel(PanelKind::Layers));
    }
}
