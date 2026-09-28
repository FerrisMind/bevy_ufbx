//! FBX loader implementation for Bevy.

use crate::error::FbxError;
use crate::material::process_materials;
use crate::mesh::process_meshes;
use crate::node::process_nodes_and_skins;
use crate::scene::build_scene;
use crate::types::{Fbx, FbxAxisSystem, FbxMeta, Handedness};
use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "animation")]
use crate::animation::process_animations;

/// Settings for FBX file loading.
#[derive(Serialize, Deserialize, Clone)]
pub struct FbxLoaderSettings {
    /// How meshes should be loaded and used.
    pub load_meshes: RenderAssetUsages,
    /// How materials should be loaded and used.
    pub load_materials: RenderAssetUsages,
    /// Whether to load cameras from the FBX file.
    pub load_cameras: bool,
    /// Whether to load lights from the FBX file.
    pub load_lights: bool,
    /// Whether to bake and load animation stacks as [`AnimationClip`]s.
    pub load_animations: bool,
    /// Whether to keep raw FBX bytes on the [`Fbx`] asset.
    pub include_source: bool,
    /// When true, force remapping into Bevy's right-handed Y-up metres space
    /// (always applied via ufbx `LoadOpts` today; kept for API compatibility).
    pub convert_coordinates: bool,
}

impl Default for FbxLoaderSettings {
    fn default() -> Self {
        Self {
            load_meshes: RenderAssetUsages::default(),
            load_materials: RenderAssetUsages::default(),
            load_cameras: true,
            load_lights: true,
            load_animations: true,
            include_source: false,
            convert_coordinates: true,
        }
    }
}

/// Loader implementation for FBX files.
#[derive(Default, bevy::reflect::TypePath)]
pub struct FbxLoader;

impl AssetLoader for FbxLoader {
    type Asset = Fbx;
    type Settings = FbxLoaderSettings;
    type Error = FbxError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Fbx, FbxError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        if bytes.is_empty() {
            return Err(FbxError::InvalidData("Empty FBX file".to_string()));
        }
        if bytes.len() < 32 {
            return Err(FbxError::InvalidData("FBX file too small".to_string()));
        }

        let load_opts = if settings.convert_coordinates {
            ufbx::LoadOpts {
                target_unit_meters: 1.0,
                target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
                space_conversion: ufbx::SpaceConversion::AdjustTransforms,
                geometry_transform_handling: ufbx::GeometryTransformHandling::HelperNodes,
                inherit_mode_handling: ufbx::InheritModeHandling::Compensate,
                generate_missing_normals: true,
                ..Default::default()
            }
        } else {
            ufbx::LoadOpts {
                generate_missing_normals: true,
                ..Default::default()
            }
        };

        let root = ufbx::load_memory(&bytes, load_opts)
            .map_err(|e| FbxError::UfbxError(format!("{e:?}")))?;
        let scene: &ufbx::Scene = &root;

        let processed_meshes = process_meshes(scene, settings, load_context)?;

        let (materials, named_materials) = if !settings.load_materials.is_empty() {
            process_materials(scene, settings, load_context)?
        } else {
            (Vec::new(), HashMap::new())
        };

        let processed_nodes =
            process_nodes_and_skins(scene, &processed_meshes.node_meshes, load_context)?;

        #[cfg(feature = "animation")]
        let processed_anims = if settings.load_animations {
            process_animations(scene, load_context)?
        } else {
            crate::animation::ProcessedAnimations {
                animations: Vec::new(),
                named_animations: HashMap::new(),
            }
        };

        #[cfg(feature = "animation")]
        let has_animations = settings.load_animations && !processed_anims.animations.is_empty();
        #[cfg(not(feature = "animation"))]
        let has_animations = false;

        let scene_handle = build_scene(
            scene,
            &processed_meshes.node_meshes,
            &materials,
            &named_materials,
            &processed_nodes.skin_data_by_mesh_element,
            has_animations,
            settings,
            load_context,
        )?;

        let metadata = FbxMeta {
            creator: Some(scene.metadata.creator.to_string()).filter(|s| !s.is_empty()),
            creation_time: None,
            original_application: {
                let app = &scene.metadata.original_application;
                let s = format!("{} {} {}", app.vendor, app.name, app.version)
                    .trim()
                    .to_string();
                if s.is_empty() { None } else { Some(s) }
            },
        };

        let (axis_system, unit_scale) = if settings.convert_coordinates {
            (
                FbxAxisSystem {
                    up: Vec3::Y,
                    front: Vec3::NEG_Z,
                    handedness: Handedness::Right,
                },
                1.0,
            )
        } else {
            let axes = scene.settings.axes;
            (
                FbxAxisSystem {
                    up: axis_to_vec3(axes.up),
                    front: axis_to_vec3(axes.front),
                    // FBX axis triples are typically right-handed; unknown → Right.
                    handedness: Handedness::Right,
                },
                scene.settings.unit_meters as f32,
            )
        };

        let source = if settings.include_source {
            Some(bytes)
        } else {
            None
        };

        Ok(Fbx {
            scenes: vec![scene_handle.clone()],
            named_scenes: HashMap::new(),
            meshes: processed_meshes.meshes,
            named_meshes: processed_meshes.named_meshes,
            materials,
            named_materials,
            nodes: processed_nodes.nodes,
            named_nodes: processed_nodes.named_nodes,
            skins: processed_nodes.skins,
            named_skins: processed_nodes.named_skins,
            #[cfg(feature = "animation")]
            animations: processed_anims.animations,
            #[cfg(feature = "animation")]
            named_animations: processed_anims.named_animations,
            default_scene: Some(scene_handle),
            axis_system,
            unit_scale,
            metadata,
            source,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["fbx"]
    }
}

fn axis_to_vec3(axis: ufbx::CoordinateAxis) -> Vec3 {
    match axis {
        ufbx::CoordinateAxis::PositiveX => Vec3::X,
        ufbx::CoordinateAxis::NegativeX => Vec3::NEG_X,
        ufbx::CoordinateAxis::PositiveY => Vec3::Y,
        ufbx::CoordinateAxis::NegativeY => Vec3::NEG_Y,
        ufbx::CoordinateAxis::PositiveZ => Vec3::Z,
        ufbx::CoordinateAxis::NegativeZ => Vec3::NEG_Z,
        ufbx::CoordinateAxis::Unknown => Vec3::Y,
    }
}
