#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use nyatidraw_api::{ContentRootId, HistoryNodeId};
use nyatidraw_brush::RecordedStroke;
use nyatidraw_stroke::StrokeCommitId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationRecord {
    Stroke {
        commit_id: StrokeCommitId,
        recorded: RecordedStroke,
    },
    StructuralChange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryNode {
    pub id: HistoryNodeId,
    pub parent: Option<HistoryNodeId>,
    pub timestamp_ns: u64,
    pub operation: OperationRecord,
    pub before_root: ContentRootId,
    pub after_root: ContentRootId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryError {
    DuplicateNode(HistoryNodeId),
    ParentMismatch,
    BeforeRootMismatch,
    NothingToUndo,
    NothingToRedo,
    AmbiguousRedo { candidates: usize },
    UnknownRedoCandidate(HistoryNodeId),
    RedoBranchMismatch,
    CorruptCursor,
    MissingParent(HistoryNodeId),
    InitialRootMismatch(HistoryNodeId),
    ParentRootMismatch(HistoryNodeId),
    Cycle(HistoryNodeId),
    MissingCursor(HistoryNodeId),
    CursorRootMismatch,
}

/// Branch-preserving history DAG cursor over immutable content roots.
///
/// Node IDs are supplied by the caller and must be unique. The history does
/// not derive IDs from timestamps or traversal order; branch candidates are
/// always exposed in ascending `HistoryNodeId` order.
#[derive(Clone, Debug)]
pub struct History {
    initial_root: ContentRootId,
    current_root: ContentRootId,
    head: Option<HistoryNodeId>,
    nodes: BTreeMap<HistoryNodeId, HistoryNode>,
    children: BTreeMap<Option<HistoryNodeId>, BTreeSet<HistoryNodeId>>,
}

impl History {
    #[must_use]
    pub fn new(initial_root: ContentRootId) -> Self {
        Self {
            initial_root,
            current_root: initial_root,
            head: None,
            nodes: BTreeMap::new(),
            children: BTreeMap::new(),
        }
    }

    /// Reconstructs a complete persisted history graph at `head`.
    ///
    /// The caller supplies a bounded collection of decoded nodes. Every node
    /// is retained, including children outside the cursor's ancestry, so redo
    /// branches remain selectable after reopening.
    ///
    /// # Errors
    ///
    /// Returns an error when identifiers repeat, a parent is missing, root
    /// transitions do not join, a cycle exists, or the requested cursor does
    /// not match the reconstructed graph.
    pub fn from_persisted(
        initial_root: ContentRootId,
        head: Option<HistoryNodeId>,
        nodes: impl IntoIterator<Item = HistoryNode>,
    ) -> Result<Self, HistoryError> {
        let mut node_map = BTreeMap::new();
        for node in nodes {
            let id = node.id;
            if node_map.insert(id, node).is_some() {
                return Err(HistoryError::DuplicateNode(id));
            }
        }

        let mut children = BTreeMap::new();
        for node in node_map.values() {
            match node.parent {
                Some(parent) => {
                    let parent_node = node_map
                        .get(&parent)
                        .ok_or(HistoryError::MissingParent(node.id))?;
                    if parent_node.after_root != node.before_root {
                        return Err(HistoryError::ParentRootMismatch(node.id));
                    }
                }
                None if node.before_root != initial_root => {
                    return Err(HistoryError::InitialRootMismatch(node.id));
                }
                None => {}
            }
            children
                .entry(node.parent)
                .or_insert_with(BTreeSet::new)
                .insert(node.id);
        }

        for start in node_map.keys().copied() {
            let mut seen = BTreeSet::new();
            let mut cursor = Some(start);
            while let Some(id) = cursor {
                if !seen.insert(id) {
                    return Err(HistoryError::Cycle(id));
                }
                cursor = node_map.get(&id).ok_or(HistoryError::CorruptCursor)?.parent;
            }
        }

        let current_root = match head {
            Some(id) => node_map
                .get(&id)
                .map(|node| node.after_root)
                .ok_or(HistoryError::MissingCursor(id))?,
            None => initial_root,
        };
        Ok(Self {
            initial_root,
            current_root,
            head,
            nodes: node_map,
            children,
        })
    }

    /// Appends one materialized operation and advances the current root.
    ///
    /// # Errors
    ///
    /// Returns an error when the node does not extend the current head/root or
    /// reuses an existing identifier.
    pub fn append(&mut self, node: HistoryNode) -> Result<(), HistoryError> {
        if self.nodes.contains_key(&node.id) {
            return Err(HistoryError::DuplicateNode(node.id));
        }
        if node.parent != self.head {
            return Err(HistoryError::ParentMismatch);
        }
        if node.before_root != self.current_root {
            return Err(HistoryError::BeforeRootMismatch);
        }
        let id = node.id;
        let parent = node.parent;
        self.current_root = node.after_root;
        self.head = Some(id);
        self.nodes.insert(id, node);
        self.children.entry(parent).or_default().insert(id);
        Ok(())
    }

    /// Moves to the parent root while retaining the node for redo.
    ///
    /// # Errors
    ///
    /// Returns `NothingToUndo` at the initial root or `CorruptCursor` if the
    /// in-memory cursor invariant has been violated.
    pub fn undo(&mut self) -> Result<ContentRootId, HistoryError> {
        let head = self.head.ok_or(HistoryError::NothingToUndo)?;
        let node = self.nodes.get(&head).ok_or(HistoryError::CorruptCursor)?;
        self.current_root = node.before_root;
        self.head = node.parent;
        Ok(self.current_root)
    }

    /// Returns the child branches available from the current cursor.
    ///
    /// IDs are sorted ascending, independent of insertion or traversal order.
    #[must_use]
    pub fn redo_candidates(&self) -> Vec<HistoryNodeId> {
        self.children
            .get(&self.head)
            .into_iter()
            .flatten()
            .copied()
            .collect()
    }

    /// Reapplies the only available child branch.
    ///
    /// # Errors
    ///
    /// Returns `NothingToRedo` when no child exists and `AmbiguousRedo` when
    /// branch selection must be made explicitly with `redo_to`.
    pub fn redo(&mut self) -> Result<ContentRootId, HistoryError> {
        let candidates = self.redo_candidates();
        match candidates.as_slice() {
            [] => Err(HistoryError::NothingToRedo),
            [candidate] => self.redo_to(*candidate),
            _ => Err(HistoryError::AmbiguousRedo {
                candidates: candidates.len(),
            }),
        }
    }

    /// Advances to one explicitly selected child branch.
    ///
    /// # Errors
    ///
    /// Returns an error when `candidate` is not a direct child of the current
    /// head or its stored root transition no longer matches the cursor.
    pub fn redo_to(&mut self, candidate: HistoryNodeId) -> Result<ContentRootId, HistoryError> {
        if !self
            .children
            .get(&self.head)
            .is_some_and(|children| children.contains(&candidate))
        {
            return Err(HistoryError::UnknownRedoCandidate(candidate));
        }
        let node = self
            .nodes
            .get(&candidate)
            .ok_or(HistoryError::CorruptCursor)?;
        if node.parent != self.head || node.before_root != self.current_root {
            return Err(HistoryError::RedoBranchMismatch);
        }
        self.current_root = node.after_root;
        self.head = Some(candidate);
        Ok(self.current_root)
    }

    #[must_use]
    pub const fn current_root(&self) -> ContentRootId {
        self.current_root
    }

    #[must_use]
    pub const fn initial_root(&self) -> ContentRootId {
        self.initial_root
    }

    #[must_use]
    pub const fn head(&self) -> Option<HistoryNodeId> {
        self.head
    }

    #[must_use]
    pub fn node(&self, id: HistoryNodeId) -> Option<&HistoryNode> {
        self.nodes.get(&id)
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u128, parent: Option<u128>, before: u128, after: u128) -> HistoryNode {
        HistoryNode {
            id: HistoryNodeId(id),
            parent: parent.map(HistoryNodeId),
            timestamp_ns: u64::try_from(id).unwrap_or(u64::MAX),
            operation: OperationRecord::StructuralChange,
            before_root: ContentRootId(before),
            after_root: ContentRootId(after),
        }
    }

    #[test]
    fn branch_selection_preserves_all_roots_and_rejects_invalid_transitions() {
        let mut history = History::new(ContentRootId(10));
        history.append(node(1, None, 10, 20)).expect("first node");
        history
            .append(node(2, Some(1), 20, 30))
            .expect("second node");
        assert_eq!(history.undo(), Ok(ContentRootId(20)));
        history
            .append(node(3, Some(1), 20, 40))
            .expect("new branch");
        assert_eq!(history.undo(), Ok(ContentRootId(20)));
        assert_eq!(
            history.redo_candidates(),
            vec![HistoryNodeId(2), HistoryNodeId(3)],
            "caller-provided IDs define deterministic branch order"
        );
        assert_eq!(
            history.redo(),
            Err(HistoryError::AmbiguousRedo { candidates: 2 })
        );
        assert_eq!(history.redo_to(HistoryNodeId(2)), Ok(ContentRootId(30)));
        assert_eq!(history.undo(), Ok(ContentRootId(20)));
        assert_eq!(history.redo_to(HistoryNodeId(3)), Ok(ContentRootId(40)));
        assert_eq!(history.node_count(), 3, "both redo branches remain stored");
        assert!(history.node(HistoryNodeId(2)).is_some());
        assert_eq!(
            history.append(node(3, Some(3), 40, 50)),
            Err(HistoryError::DuplicateNode(HistoryNodeId(3)))
        );
        assert_eq!(
            history.append(node(4, Some(1), 40, 50)),
            Err(HistoryError::ParentMismatch)
        );
        assert_eq!(
            history.append(node(4, Some(3), 999, 50)),
            Err(HistoryError::BeforeRootMismatch)
        );
    }
}
