//! Node and skin processing for FBX files.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::mesh::MAX_JOINTS;
use crate::names::{animation_root_typed_ids, node_display_name, node_typed_id};
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

    let anim_roots: std::collections::HashSet<u32> = {
        #[cfg(feature = "animation")]
        {
            animation_root_typed_ids(scene).into_iter().collect()
        }
        #[cfg(not(feature = "animation"))]
        {
            std::collections::HashSet::new()
        }
    };

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

        let mut inverse_bind_matrices = Vec::new();
        let mut joint_handles = Vec::new();
        let mut joint_element_ids = Vec::new();

        let cluster_count = skin_deformer.clusters.len();
        if cluster_count > MAX_JOINTS {
            warn!(
                "FBX skin on '{}' has {cluster_count} joints (Bevy max {MAX_JOINTS}); truncating IBM and joints together",
                node.element.name
            );
        }

        for cluster in skin_deformer.clusters.iter().take(MAX_JOINTS) {
            let bone_inverse = convert_matrix(&cluster.bind_to_world).inverse();
            let geometry_to_world = convert_matrix(&cluster.geometry_to_world);
            let ibm = bone_inverse * geometry_to_world;
            if !ibm.is_finite() {
                return Err(FbxError::ConversionError(format!(
                    "Non-finite inverse bind matrix on skin for node '{}'",
                    node.element.name
                )));
            }
            inverse_bind_matrices.push(ibm);

            if let Some(bone_node) = cluster.bone_node.as_ref() {
                joint_element_ids.push(bone_node.element.element_id);
                if let Some(joint_handle) = node_map.get(&bone_node.element.element_id) {
                    joint_handles.push(joint_handle.clone());
                } else {
                    return Err(FbxError::ConversionError(format!(
                        "Missing joint node asset for skin on '{}'",
                        node.element.name
                    )));
                }
            } else {
                return Err(FbxError::ConversionError(format!(
                    "Skin cluster without bone on node '{}'",
                    node.element.name
                )));
            }
        }

        if inverse_bind_matrices.is_empty() {
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
