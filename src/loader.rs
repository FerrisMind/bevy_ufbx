//! FBX loader implementation for Bevy.

use crate::error::FbxError;
use crate::material::process_materials_with_external_bytes;
use crate::mesh::process_meshes;
use crate::node::process_nodes_and_skins;
use crate::scene::build_scene;
use crate::types::{Fbx, FbxAxisSystem, FbxMeta, Handedness};
use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader};
use bevy::image::ImageSamplerDescriptor;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "animation")]
use crate::animation::process_animations;

/// Settings for FBX file loading.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct FbxLoaderSettings {
    /// How meshes should be loaded and used.
    pub load_meshes: RenderAssetUsages,
    /// How materials should be loaded and used.
    pub load_materials: RenderAssetUsages,
    /// Whether to load cameras from the FBX file.
    pub load_cameras: bool,
    /// Whether to load lights from the FBX file.
    pub load_lights: bool,
    /// Whether to bake and load animation stacks as Bevy `AnimationClip`s.
    pub load_animations: bool,
    /// Samples-per-second for `ufbx::bake_anim` resampling and morph-weight sampling.
    ///
    /// Maps to [`ufbx::BakeOpts::resample_rate`] (ufbx default is 30). Ignored when
    /// [`Self::load_animations`] is false.
    pub bake_fps: f32,
    /// When true (and [`Self::load_animations`] is set), append a rest/bind
    /// Bevy `AnimationClip` labeled `AnimationRest` / named `"Rest"` with a single
    /// keyframe at `t = 0` for each animated node's rest local transform (and
    /// morph weights when present). Useful for character workflows (Godot-style).
    pub generate_rest_animation: bool,
    /// Whether to keep raw FBX bytes on the [`Fbx`] asset.
    pub include_source: bool,
    /// When true, remap into Bevy right-handed Y-up metres via ufbx `LoadOpts`.
    pub convert_coordinates: bool,
    /// How unit/axis conversion is applied when [`Self::convert_coordinates`] is set.
    ///
    /// Default [`FbxSpaceConversion::Auto`] picks Maya-friendly `ModifyGeometry` or
    /// Blender-friendly `AdjustTransforms` from the file exporter metadata.
    pub space_conversion: FbxSpaceConversion,
    /// Bounds / frustum-culling policy for skinned meshes (mirrors glTF).
    pub skinned_mesh_bounds_policy: FbxSkinnedMeshBoundsPolicy,
    /// Base sampler for textures. FBX wrap modes are applied on top unless
    /// [`Self::override_sampler`] is set.
    pub default_sampler: ImageSamplerDescriptor,
    /// When set, ignore FBX wrap modes and use this sampler for every texture.
    pub override_sampler: Option<ImageSamplerDescriptor>,
}

/// How ufbx applies unit/axis conversion when coordinate conversion is enabled.
///
/// See <https://ufbx.github.io/elements/nodes/#coordinate-spaces>.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FbxSpaceConversion {
    /// Pick strategy from the file exporter (Blender → adjust transforms, else modify geometry).
    #[default]
    Auto,
    /// Bake unit scale into geometry; axis fix still adjusts transforms.
    /// Best for Maya / cm-baked meshes (Mixamo, many DCC exports).
    ModifyGeometry,
    /// Fold conversion into node TRS (often leaves `scale = 0.01` on Maya roots).
    /// Prefer for Blender FBX that applied root `×100` on export.
    AdjustTransforms,
    /// Put the whole conversion on the scene root node only.
    TransformRoot,
}

impl FbxSpaceConversion {
    fn resolve(self, exporter: ufbx::Exporter) -> ufbx::SpaceConversion {
        match self {
            Self::Auto => match exporter {
                ufbx::Exporter::BlenderBinary | ufbx::Exporter::BlenderAscii => {
                    ufbx::SpaceConversion::AdjustTransforms
                }
                _ => ufbx::SpaceConversion::ModifyGeometry,
            },
            Self::ModifyGeometry => ufbx::SpaceConversion::ModifyGeometry,
            Self::AdjustTransforms => ufbx::SpaceConversion::AdjustTransforms,
            Self::TransformRoot => ufbx::SpaceConversion::TransformRoot,
        }
    }
}

/// Controls bounds components on skinned mesh entities (same roles as glTF).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FbxSkinnedMeshBoundsPolicy {
    /// Bind-pose mesh AABB only (Bevy default mesh AABB).
    BindPose,
    /// Dynamic skinned bounds (follow animation).
    #[default]
    Dynamic,
    /// Bind-pose AABB plus disable frustum culling.
    NoFrustumCulling,
}

impl Default for FbxLoaderSettings {
    fn default() -> Self {
        Self {
            load_meshes: RenderAssetUsages::default(),
            load_materials: RenderAssetUsages::default(),
            load_cameras: true,
            load_lights: true,
            load_animations: true,
            bake_fps: 30.0,
            generate_rest_animation: false,
            include_source: false,
            convert_coordinates: true,
            space_conversion: FbxSpaceConversion::Auto,
            skinned_mesh_bounds_policy: FbxSkinnedMeshBoundsPolicy::Dynamic,
            default_sampler: ImageSamplerDescriptor::default(),
            override_sampler: None,
        }
    }
}

/// Loader implementation for FBX files.
///
/// The [`Self::default_sampler`] handle is shared with the plugin-level
/// [`crate::DefaultFbxImageSampler`] resource, so mutating that resource affects
/// subsequent loads without re-registering the loader.
#[derive(Default, bevy::reflect::TypePath)]
pub struct FbxLoader {
    /// Plugin-level default sampler, shared with [`crate::DefaultFbxImageSampler`].
    pub default_sampler: std::sync::Arc<std::sync::Mutex<ImageSamplerDescriptor>>,
}

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
        // Fold the plugin-level sampler default into the per-load settings so
        // every downstream consumer sees one effective base sampler. The
        // per-load value wins whenever it was explicitly set.
        let mut effective_settings = settings.clone();
        effective_settings.default_sampler = crate::resolve_default_sampler(
            &self.default_sampler,
            &settings.default_sampler,
        );
        let settings = &effective_settings;

        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        if bytes.is_empty() {
            return Err(FbxError::InvalidData("Empty FBX file".to_string()));
        }
        if bytes.len() < 32 {
            return Err(FbxError::InvalidData("FBX file too small".to_string()));
        }

        let exporter = {
            let probe = ufbx::load_memory(
                &bytes,
                ufbx::LoadOpts {
                    ignore_all_content: true,
                    load_external_files: false,
                    ..Default::default()
                },
            )
            .map_err(|e| FbxError::UfbxError(format!("{e:?}")))?;
            probe.metadata.exporter
        };
        let is_blender = matches!(
            exporter,
            ufbx::Exporter::BlenderBinary | ufbx::Exporter::BlenderAscii
        );

        let make_load_opts = || {
            if settings.convert_coordinates {
                let space_conversion = settings.space_conversion.resolve(exporter);
                ufbx::LoadOpts {
                    target_unit_meters: 1.0,
                    target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
                    target_camera_axes: ufbx::CoordinateAxes::right_handed_y_up(),
                    target_light_axes: ufbx::CoordinateAxes::right_handed_y_up(),
                    space_conversion,
                    geometry_transform_handling: ufbx::GeometryTransformHandling::HelperNodes,
                    inherit_mode_handling: ufbx::InheritModeHandling::Compensate,
                    generate_missing_normals: true,
                    // External texture files are read through the Bevy asset
                    // source ([`crate::material::texture`]); ufbx must not open
                    // files itself (it also governs geometry cache / OBJ mtl
                    // reads, which this loader does not support).
                    load_external_files: false,
                    // Blender exporter (Auto also resolves to AdjustTransforms for these files).
                    use_blender_pbr_material: is_blender,
                    ..Default::default()
                }
            } else {
                ufbx::LoadOpts {
                    generate_missing_normals: true,
                    load_external_files: false,
                    use_blender_pbr_material: is_blender,
                    ..Default::default()
                }
            }
        };

        let root = ufbx::load_memory(&bytes, make_load_opts())
            .map_err(|e| FbxError::UfbxError(format!("{e:?}")))?;

        // External texture bytes must be read before any material packing and
        // before the ufbx scene is held across an await (`ufbx::Scene` is not
        // `Send`, so the asset loader future must not keep it alive at an await
        // point). Requests own their paths; the scene is dropped, the files are
        // read through the load context, and the scene is parsed again only when
        // there is something to read.
        let (root, external_texture_bytes) = {
            let fbx_dir = crate::material::texture::asset_base_dir(load_context);
            let requests = crate::material::texture::external_texture_requests(&root, &fbx_dir);
            if requests.is_empty() {
                (root, HashMap::new())
            } else {
                drop(root);
                let external_bytes =
                    crate::material::texture::read_external_texture_bytes(&requests, load_context)
                        .await;
                let root = ufbx::load_memory(&bytes, make_load_opts())
                    .map_err(|e| FbxError::UfbxError(format!("{e:?}")))?;
                (root, external_bytes)
            }
        };
        let scene: &ufbx::Scene = &root;

        // Materials before meshes so [`FbxPrimitive`] can store material handles.
        let processed_materials = if !settings.load_materials.is_empty() {
            process_materials_with_external_bytes(
                scene,
                settings,
                load_context,
                &external_texture_bytes,
            )?
        } else {
            crate::material::ProcessedMaterials {
                materials: Vec::new(),
                named_materials: HashMap::new(),
                standard_materials: Vec::new(),
                named_standard_materials: HashMap::new(),
                inverted_materials: Vec::new(),
            }
        };

        let processed_meshes = process_meshes(
            scene,
            settings,
            load_context,
            &processed_materials.standard_materials,
            &processed_materials.named_standard_materials,
        )?;

        let processed_nodes =
            process_nodes_and_skins(scene, &processed_meshes.node_fbx_meshes, load_context)?;

        #[cfg(feature = "animation")]
        let processed_anims = if settings.load_animations {
            process_animations(
                scene,
                load_context,
                settings.bake_fps,
                settings.generate_rest_animation,
            )?
        } else {
            crate::animation::ProcessedAnimations {
                animations: Vec::new(),
                named_animations: HashMap::new(),
                has_curves: false,
            }
        };

        #[cfg(feature = "animation")]
        let has_animations = settings.load_animations && processed_anims.has_curves;
        #[cfg(not(feature = "animation"))]
        let has_animations = false;

        let scene_handle = build_scene(
            scene,
            &processed_meshes.node_meshes,
            &processed_materials.standard_materials,
            &processed_materials.named_standard_materials,
            &processed_materials.inverted_materials,
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
            original_unit_meters: scene.settings.original_unit_meters as f32,
            geometry_scale: scene.metadata.geometry_scale as f32,
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
            (axis_system(axes), scene.settings.unit_meters as f32)
        };

        let source = if settings.include_source {
            Some(bytes)
        } else {
            None
        };

        let mut named_scenes = HashMap::new();
        let root_name = scene.root_node.element.name.to_string();
        let scene_name = if root_name.is_empty() {
            "Scene0".to_string()
        } else {
            root_name
        };
        named_scenes.insert(Box::from(scene_name.as_str()), scene_handle.clone());

        Ok(Fbx {
            scenes: vec![scene_handle.clone()],
            named_scenes,
            meshes: processed_meshes.meshes,
            named_meshes: processed_meshes.named_meshes,
            primitive_meshes: processed_meshes.primitive_meshes,
            materials: processed_materials.materials,
            named_materials: processed_materials.named_materials,
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

fn axis_system(axes: ufbx::CoordinateAxes) -> FbxAxisSystem {
    let right = axis_to_vec3(axes.right);
    let up = axis_to_vec3(axes.up);
    let front = axis_to_vec3(axes.front);
    FbxAxisSystem {
        up,
        // ufbx calls the backward axis "front"; expose forward like converted assets.
        front: -front,
        handedness: if right.cross(up).dot(front) < 0.0 {
            Handedness::Left
        } else {
            Handedness::Right
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_axis_metadata_matches_forward_and_handedness() {
        let right = axis_system(ufbx::CoordinateAxes::right_handed_y_up());
        assert_eq!(right.up, Vec3::Y);
        assert_eq!(right.front, Vec3::NEG_Z);
        assert_eq!(right.handedness, Handedness::Right);
        let left = axis_system(ufbx::CoordinateAxes {
            right: ufbx::CoordinateAxis::PositiveX,
            up: ufbx::CoordinateAxis::PositiveY,
            front: ufbx::CoordinateAxis::NegativeZ,
        });
        assert_eq!(left.front, Vec3::Z);
        assert_eq!(left.handedness, Handedness::Left);
    }
}
