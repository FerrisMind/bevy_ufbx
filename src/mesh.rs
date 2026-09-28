//! Mesh processing: attributes, skins, morph targets, NURBS tessellation.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::FbxLoaderSettings;
use crate::types::NodeMeshPrimitive;
use crate::utils::convert_matrix;
use bevy::asset::{Handle, LoadContext};
use bevy::mesh::morph::{MorphAttributes, MorphWeights, MAX_MORPH_WEIGHTS};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use std::collections::HashMap;

/// Bevy LBS joint limit (must match [`SkinnedMesh`] / skin IBM truncation).
pub const MAX_JOINTS: usize = 256;

/// Result of mesh processing.
pub struct ProcessedMeshes {
    pub meshes: Vec<Handle<Mesh>>,
    pub named_meshes: HashMap<Box<str>, Handle<Mesh>>,
    /// Maps mesh-node element_id → material-split primitives.
    pub node_meshes: HashMap<u32, Vec<NodeMeshPrimitive>>,
}

/// Process triangle meshes and tessellated NURBS surfaces.
pub fn process_meshes(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<ProcessedMeshes, FbxError> {
    let mut meshes = Vec::new();
    let mut named_meshes = HashMap::new();
    let mut node_meshes: HashMap<u32, Vec<NodeMeshPrimitive>> = HashMap::new();
    let mut mesh_asset_index = 0usize;

    // Tessellate NURBS first so MeshRoot ownership is stable.
    let mut tessellated: Vec<(u32, String, Mat4, ufbx::MeshRoot)> = Vec::new();
    for node in scene.nodes.as_ref().iter() {
        if node.mesh.is_some() {
            continue;
        }
        let Some(attrib) = node.attrib.as_ref() else {
            continue;
        };
        let Some(nurbs) = ufbx::as_nurbs_surface(attrib) else {
            continue;
        };
        match nurbs.tessellate(ufbx::TessellateSurfaceOpts {
            span_subdivision_u: 16,
            span_subdivision_v: 16,
            ..Default::default()
        }) {
            Ok(root) => {
                tessellated.push((
                    node.element.element_id,
                    node.element.name.to_string(),
                    convert_matrix(&node.geometry_to_node),
                    root,
                ));
            }
            Err(e) => {
                warn!(
                    "Failed to tessellate NURBS on '{}': {e:?}",
                    node.element.name
                );
            }
        }
    }

    for node in scene.nodes.as_ref().iter() {
        let Some(mesh_ref) = node.mesh.as_ref() else {
            continue;
        };
        let mesh = mesh_ref.as_ref();
        mesh_asset_index = append_mesh_primitives(
            mesh,
            node.element.element_id,
            &node.element.name,
            convert_matrix(&node.geometry_to_node),
            settings,
            load_context,
            &mut meshes,
            &mut named_meshes,
            &mut node_meshes,
            mesh_asset_index,
        )?;
    }

    for (element_id, name, geometry_to_node, root) in &tessellated {
        let mesh: &ufbx::Mesh = root;
        mesh_asset_index = append_mesh_primitives(
            mesh,
            *element_id,
            name,
            *geometry_to_node,
            settings,
            load_context,
            &mut meshes,
            &mut named_meshes,
            &mut node_meshes,
            mesh_asset_index,
        )?;
    }

    Ok(ProcessedMeshes {
        meshes,
        named_meshes,
        node_meshes,
    })
}

#[allow(clippy::too_many_arguments)]
fn append_mesh_primitives(
    mesh: &ufbx::Mesh,
    element_id: u32,
    node_name: &str,
    geometry_to_node: Mat4,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
    meshes: &mut Vec<Handle<Mesh>>,
    named_meshes: &mut HashMap<Box<str>, Handle<Mesh>>,
    node_meshes: &mut HashMap<u32, Vec<NodeMeshPrimitive>>,
    mut mesh_asset_index: usize,
) -> Result<usize, FbxError> {
    if mesh.num_vertices == 0 || mesh.faces.as_ref().is_empty() {
        return Ok(mesh_asset_index);
    }

    if mesh.skin_deformers.len() > 1 {
        warn!(
            "FBX mesh on '{node_name}' has {} skin deformers; only the first is used",
            mesh.skin_deformers.len()
        );
    }

    let material_groups = group_faces_by_material(mesh);
    let mut primitives = Vec::new();

    let mut sorted_groups: Vec<_> = material_groups.into_iter().collect();
    sorted_groups.sort_by_key(|(mat_idx, _)| *mat_idx);

    for (material_idx, corner_indices) in sorted_groups {
        let (mesh_handle, morph_target_count, morph_weights) = create_mesh_from_corners(
            mesh,
            &corner_indices,
            mesh_asset_index,
            settings,
            load_context,
        )?;

        if material_idx == 0 && !node_name.is_empty() {
            named_meshes.insert(Box::from(node_name), mesh_handle.clone());
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
            morph_target_count,
            morph_weights,
        });
        meshes.push(mesh_handle);
        mesh_asset_index += 1;
    }

    if !primitives.is_empty() {
        node_meshes.insert(element_id, primitives);
    }

    Ok(mesh_asset_index)
}

/// Group triangulated face corners by material index.
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
///
/// Returns `(handle, morph_target_count, default_morph_weights)`.
pub fn create_mesh_from_corners(
    ufbx_mesh: &ufbx::Mesh,
    corners: &[u32],
    mesh_index: usize,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<(Handle<Mesh>, usize, Vec<f32>), FbxError> {
    let label = FbxAssetLabel::Mesh(mesh_index).to_string();
    let mut morph_target_count = 0usize;
    let mut morph_weights = Vec::new();

    let handle = load_context.labeled_asset_scope(label, |_| {
        let mut bevy_mesh = Mesh::new(PrimitiveTopology::TriangleList, settings.load_meshes);

        let mut positions = Vec::with_capacity(corners.len());
        let mut normals = Vec::with_capacity(corners.len());
        let mut uvs = Vec::with_capacity(corners.len());
        let mut uv1s = Vec::with_capacity(corners.len());
        let mut colors = Vec::with_capacity(corners.len());
        let mut tangents = Vec::with_capacity(corners.len());
        let mut out_indices = Vec::with_capacity(corners.len());

        let has_uv1 = ufbx_mesh.uv_sets.len() > 1;
        let uv1_set = if has_uv1 {
            Some(&ufbx_mesh.uv_sets[1])
        } else {
            None
        };
        let has_color = ufbx_mesh.vertex_color.exists;
        let has_tangent = ufbx_mesh.vertex_tangent.exists;

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
                // FBX often stores V flipped relative to Bevy/glTF.
                uvs.push([uv.x as f32, 1.0 - uv.y as f32]);
            }

            if let Some(set) = uv1_set
                && set.vertex_uv.exists
            {
                let uv = set.vertex_uv[corner];
                uv1s.push([uv.x as f32, 1.0 - uv.y as f32]);
            }

            if has_color {
                let c = ufbx_mesh.vertex_color[corner];
                colors.push([c.x as f32, c.y as f32, c.z as f32, c.w as f32]);
            }

            if has_tangent {
                let t = ufbx_mesh.vertex_tangent[corner];
                tangents.push([t.x as f32, t.y as f32, t.z as f32, 1.0]);
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
        if has_uv1 && uv1s.len() == corners.len() {
            bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1s);
        }
        if has_color {
            bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        }
        if has_tangent {
            bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
        } else if ufbx_mesh.vertex_normal.exists && ufbx_mesh.vertex_uv.exists {
            let _ = bevy_mesh.generate_tangents();
        }

        if !ufbx_mesh.skin_deformers.is_empty() {
            process_skinning_data(ufbx_mesh, corners, &mut bevy_mesh);
        }

        let (count, weights) = apply_morph_targets(ufbx_mesh, corners, &mut bevy_mesh)?;
        morph_target_count = count;
        morph_weights = weights;

        bevy_mesh.insert_indices(Indices::U32(out_indices));
        Ok::<_, FbxError>(bevy_mesh)
    })?;

    Ok((handle, morph_target_count, morph_weights))
}

/// Expand sparse blend shapes into dense Bevy morph targets (corner-expanded).
fn apply_morph_targets(
    ufbx_mesh: &ufbx::Mesh,
    corners: &[u32],
    bevy_mesh: &mut Mesh,
) -> Result<(usize, Vec<f32>), FbxError> {
    if ufbx_mesh.blend_deformers.is_empty() {
        return Ok((0, Vec::new()));
    }

    let mut channels: Vec<(&ufbx::BlendChannel, &ufbx::BlendShape)> = Vec::new();
    let mut names = Vec::new();
    let mut weights = Vec::new();

    for deformer in ufbx_mesh.blend_deformers.as_ref().iter() {
        for channel in deformer.channels.as_ref().iter() {
            let Some(shape) = channel.target_shape.as_ref() else {
                continue;
            };
            if channels.len() >= MAX_MORPH_WEIGHTS {
                warn!(
                    "FBX mesh has more than {MAX_MORPH_WEIGHTS} blend channels; truncating"
                );
                break;
            }
            let name = if channel.element.name.is_empty() {
                format!("Morph_{}", channels.len())
            } else {
                channel.element.name.to_string()
            };
            names.push(name);
            weights.push(channel.weight as f32);
            channels.push((channel, shape.as_ref()));
        }
    }

    if channels.is_empty() {
        return Ok((0, Vec::new()));
    }

    let mut morph_attrs: Vec<MorphAttributes> =
        Vec::with_capacity(channels.len() * corners.len());

    for (_channel, shape) in &channels {
        // Sparse → dense map for logical vertices.
        let mut pos_map: HashMap<u32, Vec3> = HashMap::new();
        let mut nrm_map: HashMap<u32, Vec3> = HashMap::new();
        for i in 0..shape.num_offsets {
            let vert = shape.offset_vertices[i];
            let p = shape.position_offsets[i];
            pos_map.insert(
                vert,
                Vec3::new(p.x as f32, p.y as f32, p.z as f32),
            );
            if i < shape.normal_offsets.len() {
                let n = shape.normal_offsets[i];
                nrm_map.insert(
                    vert,
                    Vec3::new(n.x as f32, n.y as f32, n.z as f32),
                );
            }
        }

        for &corner in corners {
            let logical = ufbx_mesh.vertex_indices[corner as usize];
            let position = pos_map.get(&logical).copied().unwrap_or(Vec3::ZERO);
            let normal = nrm_map.get(&logical).copied().unwrap_or(Vec3::ZERO);
            morph_attrs.push(MorphAttributes::new(position, normal, Vec3::ZERO));
        }
    }

    bevy_mesh
        .try_set_morph_targets(morph_attrs)
        .map_err(|e| FbxError::MeshConversion(format!("morph targets: {e}")))?;
    bevy_mesh.set_morph_target_names(names);

    let _ = MorphWeights::new(weights.clone(), None).map_err(|e| {
        FbxError::MeshConversion(format!("morph weights: {e}"))
    })?;

    Ok((channels.len(), weights))
}

/// Process skinning data for expanded corner vertices (top-4 weights by magnitude).
///
/// Cluster indices are clamped to [`MAX_JOINTS`].
pub fn process_skinning_data(ufbx_mesh: &ufbx::Mesh, corners: &[u32], bevy_mesh: &mut Mesh) {
    let skin_deformer = &ufbx_mesh.skin_deformers[0];
    if !matches!(
        skin_deformer.skinning_method,
        ufbx::SkinningMethod::Linear | ufbx::SkinningMethod::Rigid
    ) {
        warn!(
            "FBX skinning method {:?} is not linear-blend; Bevy will approximate with LBS",
            skin_deformer.skinning_method
        );
    }

    let cluster_count = skin_deformer.clusters.len().min(MAX_JOINTS);
    let mut joint_indices = Vec::with_capacity(corners.len());
    let mut joint_weights = Vec::with_capacity(corners.len());

    for &corner in corners {
        let logical = ufbx_mesh.vertex_indices[corner as usize] as usize;
        let mut influences: Vec<(u16, f32)> = Vec::new();

        for (cluster_index, cluster) in skin_deformer.clusters.iter().enumerate().take(cluster_count)
        {
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
