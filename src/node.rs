//! Node and skin processing for FBX files.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::mesh::{MAX_JOINTS, cluster_is_valid, compact_cluster_slots};
use crate::names::node_display_name;
#[cfg(feature = "animation")]
use crate::names::{animation_root_typed_ids, node_typed_id};
use crate::types::{FbxMesh, FbxNode, FbxSkin};
use crate::utils::{convert_matrix, convert_transform, props_to_extras};
use bevy::asset::{Handle, LoadContext};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::collections::HashMap;

/// Result of node + skin processing.
pub struct ProcessedNodes {
    pub nodes: Vec<Handle<FbxNode>>,
    pub named_nodes: HashMap<Box<str>, Handle<FbxNode>>,
    pub node_map: HashMap<u32, Handle<FbxNode>>,
    pub skins: Vec<Handle<FbxSkin>>,
    pub named_skins: HashMap<Box<str>, Handle<FbxSkin>>,
    /// Skin data keyed by mesh-node element id (for WorldAsset wiring).
    pub skin_data_by_mesh_element: HashMap<u32, FbxSkin>,
}

/// Process nodes and skins.
pub fn process_nodes_and_skins(
    scene: &ufbx::Scene,
    node_fbx_meshes: &HashMap<u32, Handle<FbxMesh>>,
    load_context: &mut LoadContext,
) -> Result<ProcessedNodes, FbxError> {
    let mut nodes = Vec::new();
    let mut named_nodes = HashMap::new();
    let mut node_map: HashMap<u32, Handle<FbxNode>> = HashMap::new();

    #[cfg(feature = "animation")]
    let anim_roots: std::collections::HashSet<u32> =
        animation_root_typed_ids(scene).into_iter().collect();

    // First pass: reserve labeled handles so children/joints can reference them
    // before the [`FbxNode`] assets are materialized (bevy_gltf pattern).
    for (index, ufbx_node) in scene.nodes.as_ref().iter().enumerate() {
        let handle: Handle<FbxNode> =
            load_context.get_label_handle(FbxAssetLabel::Node(index).to_string());
        node_map.insert(ufbx_node.element.element_id, handle);
    }

    let (skins, named_skins, skin_data_by_mesh_element, skin_handles_by_mesh_element) =
        process_skins(scene, &node_map, load_context)?;

    // Second pass: materialize nodes with children + skin filled.
    for (index, ufbx_node) in scene.nodes.as_ref().iter().enumerate() {
        let name = node_display_name(ufbx_node);
        let mesh_handle = node_fbx_meshes.get(&ufbx_node.element.element_id).cloned();
        let skin = skin_handles_by_mesh_element
            .get(&ufbx_node.element.element_id)
            .cloned();

        let children: Vec<Handle<FbxNode>> = ufbx_node
            .children
            .as_ref()
            .iter()
            .filter_map(|child| node_map.get(&child.element.element_id).cloned())
            .collect();

        let fbx_node = FbxNode {
            index,
            name: name.clone(),
            children,
            mesh: mesh_handle,
            skin,
            transform: convert_transform(&ufbx_node.local_transform),
            visible: ufbx_node.visible,
            #[cfg(feature = "animation")]
            is_animation_root: anim_roots.contains(&node_typed_id(ufbx_node)),
        };

        let handle =
            load_context.add_labeled_asset(FbxAssetLabel::Node(index).to_string(), fbx_node);

        nodes.push(handle.clone());
        if !ufbx_node.element.name.is_empty() {
            named_nodes.insert(Box::from(ufbx_node.element.name.as_ref()), handle);
        }
    }

    Ok(ProcessedNodes {
        nodes,
        named_nodes,
        node_map,
        skins,
        named_skins,
        skin_data_by_mesh_element,
    })
}

/// Process skins for skeletal animation.
type SkinProcessResult = (
    Vec<Handle<FbxSkin>>,
    HashMap<Box<str>, Handle<FbxSkin>>,
    HashMap<u32, FbxSkin>,
    HashMap<u32, Handle<FbxSkin>>,
);

pub fn process_skins(
    scene: &ufbx::Scene,
    node_map: &HashMap<u32, Handle<FbxNode>>,
    load_context: &mut LoadContext,
) -> Result<SkinProcessResult, FbxError> {
    let mut skins = Vec::new();
    let mut named_skins = HashMap::new();
    let mut skin_data_by_mesh_element = HashMap::new();
    let mut skin_handles_by_mesh_element = HashMap::new();
    let mut skin_index = 0usize;

    for node in scene.nodes.as_ref().iter() {
        let Some(mesh_ref) = &node.mesh else {
            continue;
        };
        let mesh = mesh_ref.as_ref();

        if mesh.skin_deformers.is_empty() {
            continue;
        }

        let skin_deformer = &mesh.skin_deformers[0];
        if !matches!(
            skin_deformer.skinning_method,
            ufbx::SkinningMethod::Linear | ufbx::SkinningMethod::Rigid
        ) {
            warn!(
                "FBX skin on '{}' uses {:?}; Bevy only supports linear blend skinning",
                node.element.name, skin_deformer.skinning_method
            );
        }
        if mesh.skin_deformers.len() > 1 {
            warn!(
                "FBX mesh on '{}' has {} skin deformers; only the first is imported",
                node.element.name,
                mesh.skin_deformers.len()
            );
        }

        // Shared with `mesh::process_skinning_data`: the same predicate and cap
        // produce the joint slot for every cluster, so `JOINT_INDEX` values on the
        // mesh always point at the matching IBM row here.
        let cluster_slots = compact_cluster_slots(
            skin_deformer.clusters.len(),
            |index| cluster_is_valid(&skin_deformer.clusters[index]),
            MAX_JOINTS,
        );
        let total_clusters = skin_deformer.clusters.len();
        let total_valid = skin_deformer
            .clusters
            .iter()
            .filter(|cluster| cluster_is_valid(cluster))
            .count();
        let joint_count = total_valid.min(MAX_JOINTS);

        // D5: never fail the whole file for one bad cluster — warn and skip.
        if total_clusters > total_valid {
            warn!(
                "FBX skin on '{}' has {} unusable joint cluster(s) (missing bone or non-finite inverse bind matrix); skipping them",
                node.element.name,
                total_clusters - total_valid
            );
        }
        if total_valid > MAX_JOINTS {
            warn!(
                "FBX skin on '{}' has {total_valid} usable joints (Bevy max {MAX_JOINTS}); truncating IBM and joints together",
                node.element.name
            );
        }
        if joint_count == 0 {
            warn!(
                "FBX skin on '{}' has no usable joint clusters; mesh will be imported unskinned",
                node.element.name
            );
            continue;
        }

        let mut inverse_bind_matrices = Vec::with_capacity(joint_count);
        let mut joint_handles = Vec::with_capacity(joint_count);
        let mut joint_element_ids = Vec::with_capacity(joint_count);
        let mut unresolved_joint = false;

        for (cluster_index, cluster) in skin_deformer.clusters.iter().enumerate() {
            if cluster_slots[cluster_index].is_none() {
                continue;
            }

            // Canonical inverse bind matrix. ufbx documents `geometry_to_bone` as the
            // binding matrix from mesh (geometry) vertices to the bone — the inverse
            // bind matrix — and computes `geometry_to_world = bone_node.node_to_world *
            // geometry_to_bone` (`ufbx.c`, `ufbxi_update_skin_cluster`). The previous
            // `bind_to_world⁻¹ * geometry_to_world` form therefore carried a stray
            // `bind⁻¹ * bone_load` factor whenever the file's load pose differs from its
            // bind pose (measured on the corpus; up to 2.97 relative error), and produced
            // non-finite matrices — failing the whole load — for singular bind poses.
            let ibm = convert_matrix(&cluster.geometry_to_bone);
            debug_assert!(
                ibm.is_finite(),
                "cluster_is_valid must reject non-finite inverse bind matrices"
            );

            let Some(bone_node) = cluster.bone_node.as_ref() else {
                continue;
            };
            let Some(joint_handle) = node_map.get(&bone_node.element.element_id) else {
                // `node_map` covers every node in `scene.nodes`, so a valid `bone_node`
                // ref always resolves; if one ever does not, drop the whole skin rather
                // than emit a joints/IBM list that no longer matches mesh influence slots.
                unresolved_joint = true;
                break;
            };

            inverse_bind_matrices.push(ibm);
            joint_element_ids.push(bone_node.element.element_id);
            joint_handles.push(joint_handle.clone());
        }

        if unresolved_joint || joint_handles.len() != joint_count {
            warn!(
                "FBX skin on '{}' references a joint node without an asset handle; skipping skin",
                node.element.name
            );
            continue;
        }

        let inverse_bindposes_handle = load_context.add_labeled_asset(
            FbxAssetLabel::InverseBindMatrices(skin_index).to_string(),
            SkinnedMeshInverseBindposes::from(inverse_bind_matrices),
        );

        let skin_name = if node.element.name.is_empty() {
            format!("Skin_{skin_index}")
        } else {
            format!("{}_Skin", node.element.name)
        };

        let extras = props_to_extras(&skin_deformer.element.props);
        let fbx_skin = FbxSkin {
            index: skin_index,
            name: skin_name.clone(),
            joints: joint_handles,
            joint_element_ids,
            mesh_element_id: node.element.element_id,
            inverse_bind_matrices: inverse_bindposes_handle,
            extras,
        };

        skin_data_by_mesh_element.insert(node.element.element_id, fbx_skin.clone());
        let handle =
            load_context.add_labeled_asset(FbxAssetLabel::Skin(skin_index).to_string(), fbx_skin);

        skin_handles_by_mesh_element.insert(node.element.element_id, handle.clone());
        skins.push(handle.clone());
        if !skin_name.starts_with("Skin_") {
            named_skins.insert(Box::from(skin_name.as_str()), handle);
        }
        skin_index += 1;
    }

    Ok((
        skins,
        named_skins,
        skin_data_by_mesh_element,
        skin_handles_by_mesh_element,
    ))
}
