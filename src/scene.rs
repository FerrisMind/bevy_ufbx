//! Scene building: hierarchy, skins, morphs, animation targets, lights, cameras.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::{FbxLoaderSettings, FbxSkinnedMeshBoundsPolicy};
#[cfg(feature = "animation")]
use crate::names::{animation_name_path, animation_root_typed_ids, is_ancestor_of};
use crate::names::{node_name_component, node_typed_id};
use crate::types::{
    FbxExtras, FbxMaterialExtras, FbxMaterialName, FbxMeshExtras, FbxMeshName, FbxSceneExtras,
    FbxSceneName, FbxSkin, NodeMeshPrimitive,
};
use crate::utils::{convert_matrix, convert_transform, props_to_extras};
use bevy::asset::{Handle, LoadContext};
use bevy::camera::visibility::{DynamicSkinnedMeshBounds, NoFrustumCulling};
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
    inverted_materials: &[Handle<StandardMaterial>],
    skin_data_by_mesh_element: &HashMap<u32, FbxSkin>,
    has_animations: bool,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<Handle<WorldAsset>, FbxError> {
    warn_unsupported_features(scene);

    let mut world = World::new();

    let scene_label = scene.root_node.element.name.to_string();
    let scene_name = if scene_label.is_empty() {
        "Scene0".to_string()
    } else {
        scene_label
    };
    let scene_extras = props_to_extras(&scene.metadata.scene_props)
        .or_else(|| props_to_extras(&scene.root_node.element.props));
    // Marker entity carrying scene identity (mirrors glTF scene name/extras).
    {
        let mut scene_marker = world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Hidden,
            FbxSceneName(scene_name),
        ));
        if let Some(extras) = scene_extras {
            scene_marker.insert(FbxSceneExtras {
                value: extras.value,
            });
        }
    }

    let default_material = materials.first().cloned().unwrap_or_else(|| {
        load_context.add_labeled_asset(
            FbxAssetLabel::DefaultMaterial.to_string(),
            StandardMaterial::default(),
        )
    });
    let default_inverted = inverted_materials.first().cloned().unwrap_or_else(|| {
        load_context.add_labeled_asset(
            FbxAssetLabel::MaterialInverted(0).to_string(),
            StandardMaterial {
                cull_mode: Some(bevy::render::render_resource::Face::Front),
                ..Default::default()
            },
        )
    });

    let mut element_to_entity: HashMap<u32, Entity> = HashMap::new();
    let mut typed_id_to_entity: HashMap<u32, Entity> = HashMap::new();

    // The synthetic ufbx root (`is_root`) is not a real FBX node: it exists only to
    // carry space conversion. `FbxSpaceConversion::TransformRoot` (and a custom
    // `LoadOpts::root_transform`) stores the whole unit/axis conversion in
    // `scene.root_node->local_transform` and nowhere else (ufbx
    // `ufbxi_setup_root_node` + `ufbx_space_conversion`), so skipping it silently
    // drops the requested conversion. Spawn it as a wrapper entity: real nodes keep
    // their local TRS untouched, and baked animation curves address real nodes only
    // (path policy A in `names.rs` never includes the synthetic root), so animation
    // can never overwrite the conversion. When the conversion is identity — the
    // `Auto` / `ModifyGeometry` / `AdjustTransforms` common path — no wrapper is
    // spawned and the hierarchy is exactly as before.
    let synthetic_root_entity: Option<Entity> = {
        let transform = convert_transform(&scene.root_node.local_transform);
        if transform == Transform::IDENTITY {
            None
        } else {
            Some(
                world
                    .spawn((
                        transform,
                        GlobalTransform::default(),
                        Visibility::Inherited,
                        Name::new("FbxSceneRoot"),
                    ))
                    .id(),
            )
        }
    };

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

        let mut entity_cmds =
            world.spawn((transform, GlobalTransform::default(), visibility, name));

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
        // Children of the synthetic ufbx root — and any parentless outlier nodes —
        // hang under the wrapper so the space conversion reaches them.
        let parent = match u_node.parent.as_ref() {
            Some(parent_ref) if !parent_ref.is_root => {
                typed_id_to_entity.get(&node_typed_id(parent_ref)).copied()
            }
            _ => synthetic_root_entity,
        };
        if let Some(parent) = parent {
            world.entity_mut(parent).add_child(child);
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
        // Match bevy_gltf: odd number of negative axes on **world** scale
        // (not local sx*sy*sz), so parent mirrors flip child winding too.
        let neg_scale = {
            let world = Transform::from_matrix(convert_matrix(&u_node.node_to_world));
            world.scale.is_negative_bitmask().count_ones() & 1 == 1
        };

        let mut max_morph = 0usize;
        let mut morph_weights: Vec<f32> = Vec::new();
        let mut first_mesh: Option<Handle<Mesh>> = None;

        if !u_node.element.name.is_empty() {
            world
                .entity_mut(node_entity)
                .insert(FbxMeshName(u_node.element.name.to_string()));
        }
        // Mesh extras: prefer ufbx Mesh element props, fall back to node props.
        let mesh_extras = u_node
            .mesh
            .as_ref()
            .and_then(|m| props_to_extras(&m.element.props))
            .or_else(|| props_to_extras(&u_node.element.props));
        if let Some(extras) = mesh_extras {
            world.entity_mut(node_entity).insert(FbxMeshExtras {
                value: extras.value,
            });
        }

        for primitive in primitives.iter() {
            let base_material = primitive
                .material_index
                .and_then(|scene_idx| {
                    compact_material_index(scene, scene_idx).and_then(|i| materials.get(i).cloned())
                })
                .or_else(|| {
                    named_materials
                        .get(primitive.material_name.as_str())
                        .cloned()
                })
                .or_else(|| materials.first().cloned())
                .unwrap_or_else(|| default_material.clone());
            // Double-sided materials already have `cull_mode: None`; skip the
            // cull-inverted twin (same visual, keeps the base asset identity).
            let material = if neg_scale && !material_is_double_sided(scene, primitive) {
                primitive
                    .material_index
                    .and_then(|scene_idx| {
                        compact_material_index(scene, scene_idx)
                            .and_then(|i| inverted_materials.get(i).cloned())
                    })
                    .or_else(|| {
                        materials
                            .iter()
                            .position(|h| h == &base_material)
                            .and_then(|i| inverted_materials.get(i).cloned())
                    })
                    .unwrap_or_else(|| default_inverted.clone())
            } else {
                base_material
            };

            let local = Transform::from_matrix(primitive.geometry_to_node);
            let mut mesh_entity = world.spawn((
                Mesh3d(primitive.mesh.clone()),
                MeshMaterial3d(material),
                local,
                GlobalTransform::default(),
                Visibility::Inherited,
            ));
            if !primitive.material_name.is_empty() {
                mesh_entity.insert(FbxMaterialName(primitive.material_name.clone()));
            }
            // Material extras from the matching material element, not the node.
            if let Some(extras) = material_extras_for_primitive(scene, primitive) {
                mesh_entity.insert(FbxMaterialExtras {
                    value: extras.value,
                });
            }

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
                    match settings.skinned_mesh_bounds_policy {
                        FbxSkinnedMeshBoundsPolicy::Dynamic => {
                            mesh_entity.insert(DynamicSkinnedMeshBounds);
                        }
                        FbxSkinnedMeshBoundsPolicy::NoFrustumCulling => {
                            mesh_entity.insert(NoFrustumCulling);
                        }
                        FbxSkinnedMeshBoundsPolicy::BindPose => {}
                    }
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
                    warn!(
                        "Failed to create MorphWeights on '{}': {e}",
                        u_node.element.name
                    );
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
    if !scene.cache_files.is_empty() || !scene.cache_deformers.is_empty() {
        warn!(
            "FBX contains geometry cache data ({} cache file(s), {} cache deformer(s)); not imported",
            scene.cache_files.len(),
            scene.cache_deformers.len()
        );
    }
    if !scene.lod_groups.is_empty() {
        warn!(
            "FBX contains {} LOD group(s); LOD switching is not imported",
            scene.lod_groups.len()
        );
    }
    if !scene.display_layers.is_empty() {
        warn!(
            "FBX contains {} display layer(s); display layers are not imported",
            scene.display_layers.len()
        );
    }
}

/// Map `scene.materials` index → compact index in loader `materials` /
/// `inverted_materials` vecs (which skip `element_id == 0` placeholders).
fn compact_material_index(scene: &ufbx::Scene, scene_index: usize) -> Option<usize> {
    let mut compact = 0usize;
    for (i, material) in scene.materials.as_ref().iter().enumerate() {
        if material.element.element_id == 0 {
            continue;
        }
        if i == scene_index {
            return Some(compact);
        }
        compact += 1;
    }
    None
}

/// True when the primitive's base material is double-sided (`cull_mode: None`).
fn material_is_double_sided(scene: &ufbx::Scene, primitive: &NodeMeshPrimitive) -> bool {
    if let Some(scene_idx) = primitive.material_index
        && let Some(material) = scene.materials.as_ref().get(scene_idx)
    {
        return material.features.double_sided.enabled;
    }
    if !primitive.material_name.is_empty() {
        for material in scene.materials.as_ref().iter() {
            if material.element.name.as_ref() == primitive.material_name.as_str() {
                return material.features.double_sided.enabled;
            }
        }
    }
    false
}

fn material_extras_for_primitive(
    scene: &ufbx::Scene,
    primitive: &NodeMeshPrimitive,
) -> Option<FbxExtras> {
    if let Some(scene_idx) = primitive.material_index
        && let Some(material) = scene.materials.as_ref().get(scene_idx)
        && let Some(extras) = props_to_extras(&material.element.props)
    {
        return Some(extras);
    }
    if !primitive.material_name.is_empty() {
        for material in scene.materials.as_ref().iter() {
            if material.element.name.as_ref() == primitive.material_name.as_str()
                && let Some(extras) = props_to_extras(&material.element.props)
            {
                return Some(extras);
            }
        }
    }
    None
}

/// FBX cone-angle range in degrees (Autodesk FBX SDK `KFbxLight`).
const FBX_MAX_CONE_ANGLE_DEGREES: f32 = 160.0;
/// FBX ConeAngle/OuterAngle default in degrees when the file omits the property.
/// ufbx reports the raw property (0.0 when absent); the FBX SDK default is 45.
const FBX_DEFAULT_CONE_ANGLE_DEGREES: f32 = 45.0;

/// Convert an FBX **full** cone aperture in degrees to a Bevy half-angle in radians.
fn cone_degrees_to_half_radians(degrees: f32) -> f32 {
    (degrees * 0.5).to_radians()
}

/// Resolve ufbx spot cone angles to Bevy [`SpotLight`] angles.
///
/// ufbx stores the raw FBX `InnerAngle`/`OuterAngle`/`ConeAngle` properties unchanged: **full**
/// cone aperture angles in degrees (FBX SDK: range 0..160, default 45; Blender's `io_scene_fbx`
/// maps `OuterAngle` directly onto its full-angle `spot_size`). Bevy's `SpotLight.inner_angle` /
/// `outer_angle` are **half** angles from the light axis in radians — `spot_light_clip_from_view`
/// projects `outer_angle * 2.0` as the FOV (`bevy_light::spot_light`) — so the full aperture must
/// be halved before the degree→radian conversion.
///
/// `outer_angle == 0.0` means the property was absent (ufbx default), which per the FBX SDK means
/// the default 45 degree cone; larger-than-range values are clamped so the half angle stays below
/// `PI / 2` (Bevy requires it). The inner angle is clamped into `[0, outer]`.
fn bevy_spot_cone_angles(inner_degrees: f32, outer_degrees: f32) -> (f32, f32) {
    let outer_full = if outer_degrees > 0.0 {
        outer_degrees.min(FBX_MAX_CONE_ANGLE_DEGREES)
    } else {
        FBX_DEFAULT_CONE_ANGLE_DEGREES
    };
    let outer = cone_degrees_to_half_radians(outer_full);
    let inner = cone_degrees_to_half_radians(inner_degrees.max(0.0)).min(outer);
    (inner, outer)
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
            let (inner_angle, outer_angle) =
                bevy_spot_cone_angles(light.inner_angle as f32, light.outer_angle as f32);
            entity.insert(SpotLight {
                color,
                intensity: light.intensity as f32 * 1000.0,
                shadow_maps_enabled: light.cast_shadows,
                inner_angle,
                outer_angle,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// FBX files store **full** cone aperture degrees; Bevy wants radius half-angles.
    #[test]
    fn fbx_cone_full_aperture_becomes_bevy_half_angle_radians() {
        let (inner, outer) = bevy_spot_cone_angles(0.0, 45.0);
        assert_eq!(inner, 0.0);
        assert!(
            (outer - core::f32::consts::FRAC_PI_8).abs() < 1e-6,
            "45 degree full aperture must be 22.5 degrees half = PI/8 rad, got {outer}"
        );

        let (inner, outer) = bevy_spot_cone_angles(60.0, 90.0);
        assert!(
            (outer - core::f32::consts::FRAC_PI_4).abs() < 1e-6,
            "90 degree full aperture must be 45 degrees half = PI/4 rad, got {outer}"
        );
        assert!(
            (inner - cone_degrees_to_half_radians(60.0)).abs() < 1e-6,
            "60 degree inner full aperture must be 30 degrees half, got {inner}"
        );
        assert!(inner <= outer);
    }

    /// A missing property is 0.0 in ufbx; the FBX SDK default cone (45 degrees) applies.
    #[test]
    fn missing_fbx_cone_angle_uses_sdk_default() {
        let (inner, outer) = bevy_spot_cone_angles(0.0, 0.0);
        assert_eq!(inner, 0.0);
        assert!(
            (outer - cone_degrees_to_half_radians(FBX_DEFAULT_CONE_ANGLE_DEGREES)).abs() < 1e-6,
            "missing cone angle must fall back to the FBX 45 degree default, got {outer}"
        );
        assert!(
            outer > 0.0,
            "a spot light must never get a degenerate zero cone"
        );
    }

    /// Values outside the documented FBX range are clamped; Bevy requires outer < PI/2 and inner <= outer.
    #[test]
    fn out_of_range_fbx_cone_angles_are_clamped() {
        let (_inner, outer) = bevy_spot_cone_angles(0.0, 350.0);
        assert!(
            outer < core::f32::consts::FRAC_PI_2,
            "clamped outer half-angle must stay below PI/2, got {outer}"
        );
        assert!((outer - cone_degrees_to_half_radians(160.0)).abs() < 1e-6);

        let (inner, outer) = bevy_spot_cone_angles(120.0, 45.0);
        assert_eq!(inner, outer, "inner must not exceed outer");

        let (inner, _) = bevy_spot_cone_angles(-10.0, 45.0);
        assert_eq!(inner, 0.0, "negative inner angle clamps to zero");
    }
}
