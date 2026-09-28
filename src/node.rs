//! Node and skin processing for FBX files.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::names::node_display_name;
use crate::types::{FbxNode, FbxSkin, NodeMeshPrimitive};
use crate::utils::{convert_matrix, convert_transform};
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
    node_meshes: &HashMap<u32, Vec<NodeMeshPrimitive>>,
    load_context: &mut LoadContext,
) -> Result<ProcessedNodes, FbxError> {
    let mut nodes = Vec::new();
    let mut named_nodes = HashMap::new();
    let mut node_map = HashMap::new();
    let mut handles_by_index: Vec<Handle<FbxNode>> = Vec::with_capacity(scene.nodes.len());

    for (index, ufbx_node) in scene.nodes.as_ref().iter().enumerate() {
        let name = node_display_name(ufbx_node);
        let mesh_handle = node_meshes
            .get(&ufbx_node.element.element_id)
            .and_then(|prims| prims.first())
            .map(|p| p.mesh.clone());

        let fbx_node = FbxNode {
            index,
            name: name.clone(),
            children: Vec::new(),
            mesh: mesh_handle,
            skin: None,
            transform: convert_transform(&ufbx_node.local_transform),
            visible: ufbx_node.visible,
        };

        let handle =
            load_context.add_labeled_asset(FbxAssetLabel::Node(index).to_string(), fbx_node);

        node_map.insert(ufbx_node.element.element_id, handle.clone());
        handles_by_index.push(handle.clone());
        nodes.push(handle.clone());

        if !ufbx_node.element.name.is_empty() {
            named_nodes.insert(Box::from(ufbx_node.element.name.as_ref()), handle);
        }
    }

    // Fill children using parent links (second labeled assets with updated data).
    // We cannot mutate existing assets; store hierarchy only in the WorldAsset.
    let _ = handles_by_index;

    let (skins, named_skins, skin_data_by_mesh_element) =
        process_skins(scene, &node_map, load_context)?;

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
);

pub fn process_skins(
    scene: &ufbx::Scene,
    node_map: &HashMap<u32, Handle<FbxNode>>,
    load_context: &mut LoadContext,
) -> Result<SkinProcessResult, FbxError> {
    let mut skins = Vec::new();
    let mut named_skins = HashMap::new();
    let mut skin_data_by_mesh_element = HashMap::new();
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
        let mut inverse_bind_matrices = Vec::new();
        let mut joint_handles = Vec::new();
        let mut joint_element_ids = Vec::new();

        for cluster in &skin_deformer.clusters {
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

        let fbx_skin = FbxSkin {
            index: skin_index,
            name: skin_name.clone(),
            joints: joint_handles,
            joint_element_ids,
            mesh_element_id: node.element.element_id,
            inverse_bind_matrices: inverse_bindposes_handle,
        };

        skin_data_by_mesh_element.insert(node.element.element_id, fbx_skin.clone());
        let handle =
            load_context.add_labeled_asset(FbxAssetLabel::Skin(skin_index).to_string(), fbx_skin);

        skins.push(handle.clone());
        if !skin_name.starts_with("Skin_") {
            named_skins.insert(Box::from(skin_name.as_str()), handle);
        }
        skin_index += 1;
    }

    Ok((skins, named_skins, skin_data_by_mesh_element))
}
