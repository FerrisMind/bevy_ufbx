//! Stable node naming and animation path helpers (path-root policy A).

use bevy::prelude::Name;

/// Typed id used to index `scene.nodes` (ufbx 0.9 stores it on `element`).
#[inline]
pub fn node_typed_id(node: &ufbx::Node) -> u32 {
    node.element.typed_id
}

/// Display name for an FBX node. Empty non-root nodes get `Node_{typed_id}`.
pub fn node_display_name(node: &ufbx::Node) -> String {
    if !node.element.name.is_empty() {
        node.element.name.to_string()
    } else if node.is_root {
        "<fbx-root>".to_string()
    } else {
        format!("Node_{}", node_typed_id(node))
    }
}

/// [`Name`] component value for an FBX node.
pub fn node_name_component(node: &ufbx::Node) -> Name {
    Name::new(node_display_name(node))
}

/// Build animation target path names from armature root → node (policy A).
///
/// The synthetic ufbx conversion root (`is_root`) is excluded so Mixamo-style
/// paths start at the meaningful armature root.
pub fn animation_name_path(scene: &ufbx::Scene, typed_id: u32) -> Vec<Name> {
    let mut chain = Vec::new();
    let mut current = Some(typed_id);

    while let Some(id) = current {
        let Some(node) = scene.nodes.get(id as usize) else {
            break;
        };
        if node.is_root {
            break;
        }
        chain.push(node_name_component(node));
        current = node.parent.as_ref().map(|p| node_typed_id(p));
    }

    chain.reverse();
    chain
}

/// Typed ids of top-level armature roots (children of the synthetic ufbx root,
/// or all nodes without a non-root parent when no synthetic root exists).
pub fn animation_root_typed_ids(scene: &ufbx::Scene) -> Vec<u32> {
    let has_synthetic_root = scene.nodes.as_ref().iter().any(|n| n.is_root);

    if has_synthetic_root {
        scene
            .nodes
            .as_ref()
            .iter()
            .filter(|n| !n.is_root && n.parent.as_ref().map(|p| p.is_root).unwrap_or(false))
            .map(|n| node_typed_id(n))
            .collect()
    } else {
        scene
            .nodes
            .as_ref()
            .iter()
            .filter(|n| n.parent.is_none())
            .map(|n| node_typed_id(n))
            .collect()
    }
}

/// True if `ancestor` is an ancestor of `node` (inclusive).
pub fn is_ancestor_of(scene: &ufbx::Scene, ancestor: u32, node: u32) -> bool {
    let mut current = Some(node);
    while let Some(typed_id) = current {
        if typed_id == ancestor {
            return true;
        }
        let Some(n) = scene.nodes.get(typed_id as usize) else {
            return false;
        };
        if n.is_root {
            return false;
        }
        current = n.parent.as_ref().map(|p| node_typed_id(p));
    }
    false
}
