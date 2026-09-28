//! Scene building: hierarchy, skins, morphs, animation targets, lights, cameras.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::FbxLoaderSettings;
use crate::names::{
    animation_name_path, animation_root_typed_ids, is_ancestor_of, node_name_component,
    node_typed_id,
};
use crate::types::{FbxExtras, FbxMeshName, FbxSkin, NodeMeshPrimitive};
use crate::utils::convert_transform;
use bevy::asset::{Handle, LoadContext};
use bevy::mesh::morph::{MeshMorphWeights, MorphWeights};
use bevy::mesh::skinning::SkinnedMesh;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use std::collections::HashMap;

#[cfg(feature = "animation")]
use bevy::animation::{AnimatedBy, AnimationPlayer, AnimationTargetId};

/// Build the final scene with a parented node hierarchy.
#[allow(clippy::too_many_arguments)]
pub fn build_scene(
    scene: &ufbx::Scene,
    node_meshes: &HashMap<u32, Vec<NodeMeshPrimitive>>,
    materials: &[Handle<StandardMaterial>],
    named_materials: &HashMap<Box<str>, Handle<StandardMaterial>>,
    skin_data_by_mesh_element: &HashMap<u32, FbxSkin>,
    has_animations: bool,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<Handle<WorldAsset>, FbxError> {
    warn_unsupported_features(scene);

    let mut world = World::new();

    let default_material = materials.first().cloned().unwrap_or_else(|| {
        load_context.add_labeled_asset(
            FbxAssetLabel::DefaultMaterial.to_string(),
            StandardMaterial::default(),
        )
    });

    let mut element_to_entity: HashMap<u32, Entity> = HashMap::new();
    let mut typed_id_to_entity: HashMap<u32, Entity> = HashMap::new();

    for u_node in scene.nodes.as_ref().iter() {
        if u_node.is_root {
            continue;
        }

        let name = node_name_component(u_node);
        let transform = convert_transform(&u_node.local_transform);
        let visibility = if u_node.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

        let mut entity_cmds = world.spawn((
            transform,
            GlobalTransform::default(),
            visibility,
            name,
        ));

        if let Some(extras) = props_to_extras(&u_node.element.props) {
            entity_cmds.insert(extras);
        }

        let entity = entity_cmds.id();
        element_to_entity.insert(u_node.element.element_id, entity);
        typed_id_to_entity.insert(node_typed_id(u_node), entity);
    }

    for u_node in scene.nodes.as_ref().iter() {
        if u_node.is_root {
            continue;
        }
        let Some(&child) = typed_id_to_entity.get(&node_typed_id(u_node)) else {
            continue;
        };
        if let Some(parent_ref) = u_node.parent.as_ref() {
            if parent_ref.is_root {
                continue;
            }
            if let Some(&parent) = typed_id_to_entity.get(&node_typed_id(parent_ref)) {
                world.entity_mut(parent).add_child(child);
            }
        }
    }

    #[cfg(feature = "animation")]
    if has_animations {
        let roots = animation_root_typed_ids(scene);
        let mut animation_players: HashMap<u32, Entity> = HashMap::new();
        for root_typed_id in roots {
            if let Some(&entity) = typed_id_to_entity.get(&root_typed_id) {
                world.entity_mut(entity).insert(AnimationPlayer::default());
                animation_players.insert(root_typed_id, entity);
            }
        }

        for u_node in scene.nodes.as_ref().iter() {
            if u_node.is_root {
                continue;
            }
            let typed_id = node_typed_id(u_node);
            let Some(&entity) = typed_id_to_entity.get(&typed_id) else {
                continue;
            };

            let path = animation_name_path(scene, typed_id);
            if path.is_empty() {
                continue;
            }

            let Some((&_root, &player_entity)) = animation_players
                .iter()
                .find(|&(&root, _)| is_ancestor_of(scene, root, typed_id))
            else {
                continue;
            };

            world.entity_mut(entity).insert((
                AnimationTargetId::from_names(path.iter()),
                AnimatedBy(player_entity),
            ));
        }
    }

    #[cfg(not(feature = "animation"))]
    let _ = has_animations;

    for u_node in scene.nodes.as_ref().iter() {
        if u_node.is_root {
            continue;
        }
        let Some(&node_entity) = element_to_entity.get(&u_node.element.element_id) else {
            continue;
        };
        let Some(primitives) = node_meshes.get(&u_node.element.element_id) else {
            continue;
        };

        let skin = skin_data_by_mesh_element.get(&u_node.element.element_id);
        let neg_scale = {
            let s = u_node.local_transform.scale;
            (s.x * s.y * s.z) < 0.0
        };
        if neg_scale {
            warn!(
                "FBX node '{}' has negative scale; cull inversion for mirrored materials is approximate",
                u_node.element.name
            );
        }

        let mut max_morph = 0usize;
        let mut morph_weights: Vec<f32> = Vec::new();
        let mut first_mesh: Option<Handle<Mesh>> = None;

        if !u_node.element.name.is_empty() {
            world
                .entity_mut(node_entity)
                .insert(FbxMeshName(u_node.element.name.to_string()));
        }

        for primitive in primitives {
            let material = named_materials
                .get(primitive.material_name.as_str())
                .cloned()
                .or_else(|| materials.first().cloned())
                .unwrap_or_else(|| default_material.clone());

            let local = Transform::from_matrix(primitive.geometry_to_node);
            let mut mesh_entity = world.spawn((
                Mesh3d(primitive.mesh.clone()),
                MeshMaterial3d(material),
                local,
                GlobalTransform::default(),
                Visibility::Inherited,
            ));

            if primitive.morph_target_count > 0 {
                max_morph = max_morph.max(primitive.morph_target_count);
                if morph_weights.len() < primitive.morph_weights.len() {
                    morph_weights = primitive.morph_weights.clone();
                }
                if first_mesh.is_none() {
                    first_mesh = Some(primitive.mesh.clone());
                }
                mesh_entity.insert(MeshMorphWeights::Reference(node_entity));
            }

            if let Some(skin) = skin {
                let mut joints = Vec::with_capacity(skin.joint_element_ids.len());
                let mut missing = false;
                for &joint_eid in &skin.joint_element_ids {
                    if let Some(&joint_entity) = element_to_entity.get(&joint_eid) {
                        joints.push(joint_entity);
                    } else {
                        missing = true;
                        break;
                    }
                }
                if !missing && !joints.is_empty() {
                    mesh_entity.insert(SkinnedMesh {
                        inverse_bindposes: skin.inverse_bind_matrices.clone(),
                        joints,
                    });
                }
            }

            let mesh_id = mesh_entity.id();
            world.entity_mut(node_entity).add_child(mesh_id);
        }

        if max_morph > 0 {
            if morph_weights.len() < max_morph {
                morph_weights.resize(max_morph, 0.0);
            }
            match MorphWeights::new(morph_weights, first_mesh) {
                Ok(mw) => {
                    world.entity_mut(node_entity).insert(mw);
                }
                Err(e) => {
                    warn!("Failed to create MorphWeights on '{}': {e}", u_node.element.name);
                }
            }
        }
    }

    for u_node in scene.nodes.as_ref().iter() {
        if u_node.is_root {
            continue;
        }
        let Some(&entity) = element_to_entity.get(&u_node.element.element_id) else {
            continue;
        };

        if settings.load_lights
            && let Some(light_ref) = u_node.light.as_ref()
        {
            insert_light(world.entity_mut(entity), light_ref.as_ref());
        }

        if settings.load_cameras
            && let Some(camera_ref) = u_node.camera.as_ref()
        {
            insert_camera(world.entity_mut(entity), camera_ref.as_ref());
        }
    }

    let scene_handle =
        load_context.add_labeled_asset(FbxAssetLabel::Scene(0).to_string(), WorldAsset::new(world));

    Ok(scene_handle)
}

fn warn_unsupported_features(scene: &ufbx::Scene) {
    if !scene.constraints.is_empty() {
        warn!(
            "FBX contains {} constraint(s); Bevy has no constraint solver — bake animation in DCC or rely on bake_anim TRS",
            scene.constraints.len()
        );
    }
}

fn props_to_extras(props: &ufbx::Props) -> Option<FbxExtras> {
    if props.props.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for prop in props.props.as_ref().iter() {
        if prop.name.is_empty() {
            continue;
        }
        let value = if !prop.value_str.is_empty() {
            prop.value_str.to_string()
        } else {
            format!(
                "({},{},{},{})",
                prop.value_vec4.x, prop.value_vec4.y, prop.value_vec4.z, prop.value_vec4.w
            )
        };
        parts.push(format!("{}={}", prop.name, value));
    }
    if parts.is_empty() {
        None
    } else {
        Some(FbxExtras {
            value: parts.join(";"),
        })
    }
}

fn insert_light(mut entity: EntityWorldMut, light: &ufbx::Light) {
    let color = Color::srgb(
        light.color.x as f32,
        light.color.y as f32,
        light.color.z as f32,
    );
    match light.type_ {
        ufbx::LightType::Directional => {
            entity.insert(DirectionalLight {
                color,
                illuminance: light.intensity as f32 * 10000.0,
                shadow_maps_enabled: light.cast_shadows,
                ..Default::default()
            });
        }
        ufbx::LightType::Point => {
            entity.insert(PointLight {
                color,
                intensity: light.intensity as f32 * 1000.0,
                shadow_maps_enabled: light.cast_shadows,
                ..Default::default()
            });
        }
        ufbx::LightType::Spot => {
            entity.insert(SpotLight {
                color,
                intensity: light.intensity as f32 * 1000.0,
                shadow_maps_enabled: light.cast_shadows,
                inner_angle: light.inner_angle as f32,
                outer_angle: light.outer_angle as f32,
                ..Default::default()
            });
        }
        ufbx::LightType::Area => {
            warn!("FBX area light approximated as PointLight");
            entity.insert(PointLight {
                color,
                intensity: light.intensity as f32 * 1000.0,
                shadow_maps_enabled: light.cast_shadows,
                ..Default::default()
            });
        }
        _ => {
            warn!("Unsupported FBX light type {:?}", light.type_);
        }
    }
}

fn insert_camera(mut entity: EntityWorldMut, camera: &ufbx::Camera) {
    let fov_y = camera.field_of_view_deg.y as f32;
    let projection = match camera.projection_mode {
        ufbx::ProjectionMode::Orthographic => {
            let mag = camera.orthographic_extent as f32;
            Projection::Orthographic(OrthographicProjection {
                near: camera.near_plane as f32,
                far: camera.far_plane as f32,
                scaling_mode: bevy::camera::ScalingMode::FixedHorizontal {
                    viewport_width: if mag > 0.0 { mag * 2.0 } else { 1.0 },
                },
                ..OrthographicProjection::default_3d()
            })
        }
        _ => {
            let mut perspective = PerspectiveProjection {
                fov: fov_y.to_radians(),
                near: camera.near_plane as f32,
                ..Default::default()
            };
            if camera.far_plane > 0.0 {
                perspective.far = camera.far_plane as f32;
            }
            if camera.aspect_ratio > 0.0 {
                perspective.aspect_ratio = camera.aspect_ratio as f32;
            }
            Projection::Perspective(perspective)
        }
    };

    entity.insert((
        Camera3d::default(),
        projection,
        Camera {
            is_active: false,
            ..Default::default()
        },
    ));
}
