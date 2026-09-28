//! Mesh processing functionality for FBX files.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::FbxLoaderSettings;
use crate::types::NodeMeshPrimitive;
use crate::utils::convert_matrix;
use bevy::asset::{Handle, LoadContext};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use std::collections::HashMap;

/// Result of mesh processing.
pub struct ProcessedMeshes {
    pub meshes: Vec<Handle<Mesh>>,
    pub named_meshes: HashMap<Box<str>, Handle<Mesh>>,
    /// Maps mesh-node element_id → material-split primitives.
    pub node_meshes: HashMap<u32, Vec<NodeMeshPrimitive>>,
}

/// Process all meshes from the FBX scene.
pub fn process_meshes(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<ProcessedMeshes, FbxError> {
    let mut meshes = Vec::new();
    let mut named_meshes = HashMap::new();
    let mut node_meshes: HashMap<u32, Vec<NodeMeshPrimitive>> = HashMap::new();
    let mut mesh_asset_index = 0usize;

    for node in scene.nodes.as_ref().iter() {
        let Some(mesh_ref) = node.mesh.as_ref() else {
            continue;
        };
        let mesh = mesh_ref.as_ref();

        if mesh.num_vertices == 0 || mesh.faces.as_ref().is_empty() {
            continue;
        }

        let material_groups = group_faces_by_material(mesh);
        let geometry_to_node = convert_matrix(&node.geometry_to_node);
        let mut primitives = Vec::new();

        let mut sorted_groups: Vec<_> = material_groups.into_iter().collect();
        sorted_groups.sort_by_key(|(mat_idx, _)| *mat_idx);

        for (material_idx, corner_indices) in sorted_groups {
            let mesh_handle = create_mesh_from_corners(
                mesh,
                &corner_indices,
                mesh_asset_index,
                settings,
                load_context,
            )?;

            if material_idx == 0 && !node.element.name.is_empty() {
                named_meshes.insert(Box::from(node.element.name.as_ref()), mesh_handle.clone());
            }

            let material_name = if material_idx < mesh.materials.len() {
                mesh.materials[material_idx].element.name.to_string()
            } else {
                "default".to_string()
            };

            primitives.push(NodeMeshPrimitive {
                mesh: mesh_handle.clone(),
                material_name,
                geometry_to_node,
            });
            meshes.push(mesh_handle);
            mesh_asset_index += 1;
        }

        if !primitives.is_empty() {
            node_meshes.insert(node.element.element_id, primitives);
        }
    }

    Ok(ProcessedMeshes {
        meshes,
        named_meshes,
        node_meshes,
    })
}

/// Group triangulated face corners by material index.
///
/// Returns corner indices into the ufbx mesh (for `vertex_*[corner]` accessors),
/// not logical vertex indices.
pub fn group_faces_by_material(mesh: &ufbx::Mesh) -> HashMap<usize, Vec<u32>> {
    let mut material_groups: HashMap<usize, Vec<u32>> = HashMap::new();
    let mut scratch = Vec::new();

    for (face_idx, &face) in mesh.faces.as_ref().iter().enumerate() {
        let material_idx = if mesh.materials.is_empty() {
            0
        } else if !mesh.face_material.is_empty() && face_idx < mesh.face_material.len() {
            mesh.face_material[face_idx] as usize
        } else {
            0
        };

        scratch.clear();
        ufbx::triangulate_face_vec(&mut scratch, mesh, face);
        material_groups
            .entry(material_idx)
            .or_default()
            .extend(scratch.iter().copied());
    }

    material_groups
}

/// Create a Bevy mesh from triangulated corner indices.
pub fn create_mesh_from_corners(
    ufbx_mesh: &ufbx::Mesh,
    corners: &[u32],
    mesh_index: usize,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<Handle<Mesh>, FbxError> {
    let label = FbxAssetLabel::Mesh(mesh_index).to_string();

    let handle = load_context.labeled_asset_scope(label, |_| {
        let mut bevy_mesh = Mesh::new(PrimitiveTopology::TriangleList, settings.load_meshes);

        let mut positions = Vec::with_capacity(corners.len());
        let mut normals = Vec::with_capacity(corners.len());
        let mut uvs = Vec::with_capacity(corners.len());
        let mut out_indices = Vec::with_capacity(corners.len());

        for (out_i, &corner) in corners.iter().enumerate() {
            let corner = corner as usize;
            let p = ufbx_mesh.vertex_position[corner];
            positions.push([p.x as f32, p.y as f32, p.z as f32]);

            if ufbx_mesh.vertex_normal.exists {
                let n = ufbx_mesh.vertex_normal[corner];
                normals.push([n.x as f32, n.y as f32, n.z as f32]);
            }

            if ufbx_mesh.vertex_uv.exists {
                let uv = ufbx_mesh.vertex_uv[corner];
                uvs.push([uv.x as f32, uv.y as f32]);
            }

            out_indices.push(out_i as u32);
        }

        bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        if ufbx_mesh.vertex_normal.exists {
            bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        }
        if ufbx_mesh.vertex_uv.exists {
            bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        }

        if !ufbx_mesh.skin_deformers.is_empty() {
            process_skinning_data(ufbx_mesh, corners, &mut bevy_mesh);
        }

        bevy_mesh.insert_indices(Indices::U32(out_indices));
        Ok::<_, FbxError>(bevy_mesh)
    })?;

    Ok(handle)
}

/// Process skinning data for expanded corner vertices (top-4 weights by magnitude).
pub fn process_skinning_data(ufbx_mesh: &ufbx::Mesh, corners: &[u32], bevy_mesh: &mut Mesh) {
    let skin_deformer = &ufbx_mesh.skin_deformers[0];
    let mut joint_indices = Vec::with_capacity(corners.len());
    let mut joint_weights = Vec::with_capacity(corners.len());

    for &corner in corners {
        let logical = ufbx_mesh.vertex_indices[corner as usize] as usize;
        let mut influences: Vec<(u16, f32)> = Vec::new();

        for (cluster_index, cluster) in skin_deformer.clusters.iter().enumerate() {
            for (i, &vert_idx) in cluster.vertices.iter().enumerate() {
                if vert_idx as usize == logical && i < cluster.weights.len() {
                    let weight = cluster.weights[i] as f32;
                    if weight.is_finite() && weight > 0.0 {
                        influences.push((cluster_index as u16, weight));
                    }
                    break;
                }
            }
        }

        influences.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        influences.truncate(4);

        let total: f32 = influences.iter().map(|(_, w)| *w).sum();
        let mut indices = [0u16; 4];
        let mut weights = [0.0f32; 4];
        for (slot, (ji, w)) in influences.into_iter().enumerate() {
            indices[slot] = ji;
            weights[slot] = if total > 0.0 { w / total } else { 0.0 };
        }

        joint_indices.push(indices);
        joint_weights.push(weights);
    }

    bevy_mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(joint_indices),
    );
    bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_weights);
}
