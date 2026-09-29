//! Type definitions for the FBX loader.

use bevy::asset::{Asset, Handle};
use bevy::math::Affine2;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;
use bevy::reflect::Reflect;
use bevy::world_serialization::WorldAsset;
use std::collections::HashMap;

#[cfg(feature = "animation")]
use bevy::animation::AnimationClip;

// ============================================================================
// Coordinate System
// ============================================================================

/// Handedness of a coordinate system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handedness {
    Right,
    Left,
}

/// Coordinate axes definition.
#[derive(Debug, Clone, Copy)]
pub struct FbxAxisSystem {
    pub up: Vec3,
    pub front: Vec3,
    pub handedness: Handedness,
}

// ============================================================================
// Metadata
// ============================================================================

/// Metadata from FBX header.
#[derive(Debug, Clone, Default)]
pub struct FbxMeta {
    pub creator: Option<String>,
    pub creation_time: Option<String>,
    pub original_application: Option<String>,
    /// Original file unit in metres (often `0.01` for Maya cm), before conversion.
    pub original_unit_meters: f32,
    /// Geometry bake scale applied by ufbx (`ModifyGeometry`); `1.0` otherwise.
    pub geometry_scale: f32,
}

// ============================================================================
// Textures and Materials
// ============================================================================

/// Texture wrapping modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbxWrapMode {
    Repeat,
    Clamp,
}

/// Types of textures in FBX materials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FbxTextureType {
    BaseColor,
    Normal,
    Metallic,
    Roughness,
    Emission,
    AmbientOcclusion,
    Height,
}

/// Texture information.
#[derive(Debug, Clone)]
pub struct FbxTexture {
    pub name: String,
    pub filename: String,
    pub absolute_filename: String,
    pub uv_set: String,
    pub uv_transform: Affine2,
    pub wrap_u: FbxWrapMode,
    pub wrap_v: FbxWrapMode,
}

/// FBX material container (glTF-style): wraps the Bevy [`StandardMaterial`].
///
/// Labeled `Material{N}`. Cull-inverted twins used for negative-scale nodes are
/// labeled `Material{N} (inverted)` and remain bare [`StandardMaterial`] assets
/// (not wrapped in [`FbxMaterial`]).
#[derive(Asset, Debug, Clone, TypePath)]
pub struct FbxMaterial {
    /// Index in the FBX material list (`ufbx::Scene::materials`).
    pub index: usize,
    /// Display name (FBX name, or `FbxMaterial{index}` when unnamed).
    pub name: String,
    /// Non-inverted Bevy material used for normal-scale mesh entities.
    pub material: Handle<StandardMaterial>,
    /// Optional custom-property extras blob.
    pub extras: Option<FbxExtras>,
}

// ============================================================================
// Lights and Cameras
// ============================================================================

/// Light types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbxLightType {
    Directional,
    Point,
    Spot,
    Area,
    Volume,
}

/// Light definition.
#[derive(Debug, Clone)]
pub struct FbxLight {
    pub name: String,
    pub light_type: FbxLightType,
    pub color: Color,
    pub intensity: f32,
    pub cast_shadows: bool,
    pub inner_angle: Option<f32>,
    pub outer_angle: Option<f32>,
}

/// Camera projection modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbxProjectionMode {
    Perspective,
    Orthographic,
}

/// Camera definition.
#[derive(Debug, Clone)]
pub struct FbxCamera {
    pub name: String,
    pub projection_mode: FbxProjectionMode,
    pub field_of_view_deg: f32,
    pub aspect_ratio: f32,
    pub near_plane: f32,
    pub far_plane: f32,
    pub focal_length_mm: f32,
}

// ============================================================================
// Scene Elements
// ============================================================================

/// One material-group primitive of an [`FbxMesh`] (glTF `GltfPrimitive` parity).
#[derive(Debug, Clone)]
pub struct FbxPrimitive {
    pub mesh: Handle<Mesh>,
    pub material: Option<Handle<StandardMaterial>>,
    /// glTF-parity primitive name in the `"{mesh}.{material}"` form (see
    /// [`FbxPrimitive::name_for`]); `None` when the loader did not name the
    /// primitive. Also used as the `Name` component of the primitive entity in
    /// the spawned scene.
    pub name: Option<String>,
    pub extras: Option<FbxExtras>,
}

impl FbxPrimitive {
    /// Builds the glTF-parity primitive name: `"{mesh}.{material}"`, falling
    /// back to the mesh name when the primitive has no material (or the
    /// material is unnamed). Mirrors `bevy_gltf`'s `primitive_name`
    /// (`loader/gltf_ext/mesh.rs`), which backs the `Name` component of the
    /// primitive entity.
    ///
    /// Empty FBX names count as unnamed: an empty `mesh_name` falls back to
    /// `"Mesh"` and an empty material name to no material.
    pub fn name_for(mesh_name: &str, material_name: Option<&str>) -> String {
        let mesh_name = if mesh_name.is_empty() {
            "Mesh"
        } else {
            mesh_name
        };
        match material_name.filter(|name| !name.is_empty()) {
            Some(material_name) => format!("{mesh_name}.{material_name}"),
            None => mesh_name.to_string(),
        }
    }
}

/// FBX mesh container (glTF-style): one or more Bevy [`Mesh`] primitives.
#[derive(Asset, Debug, Clone, TypePath)]
pub struct FbxMesh {
    pub index: usize,
    pub name: String,
    pub primitives: Vec<FbxPrimitive>,
    pub extras: Option<FbxExtras>,
}

/// FBX node with hierarchy.
#[derive(Asset, Debug, Clone, TypePath)]
pub struct FbxNode {
    pub index: usize,
    pub name: String,
    pub children: Vec<Handle<FbxNode>>,
    pub mesh: Option<Handle<FbxMesh>>,
    pub skin: Option<Handle<FbxSkin>>,
    pub transform: Transform,
    pub visible: bool,
    /// True when this node hosts an [`AnimationPlayer`] in the spawned scene.
    #[cfg(feature = "animation")]
    pub is_animation_root: bool,
}

/// FBX skin for skeletal animation.
#[derive(Asset, Debug, Clone, TypePath)]
pub struct FbxSkin {
    pub index: usize,
    pub name: String,
    /// Joint node asset handles in cluster order.
    pub joints: Vec<Handle<FbxNode>>,
    /// Joint ufbx element IDs in cluster order (for scene entity resolution).
    pub joint_element_ids: Vec<u32>,
    /// Element id of the mesh node this skin belongs to.
    pub mesh_element_id: u32,
    pub inverse_bind_matrices: Handle<SkinnedMeshInverseBindposes>,
    /// Optional custom-property extras blob (glTF `GltfSkin::extras` parity).
    pub extras: Option<FbxExtras>,
}

/// Placeholder for skeleton data.
#[derive(Asset, Debug, Clone, TypePath)]
pub struct Skeleton;

/// Animation interpolation modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbxInterpolation {
    Constant,
    Linear,
    Cubic,
}

/// One mesh primitive attached to an FBX node (after material splitting).
#[derive(Debug, Clone)]
pub struct NodeMeshPrimitive {
    pub mesh: Handle<Mesh>,
    /// Parent FBX mesh index (for [`crate::FbxAssetLabel::Primitive`]).
    pub mesh_index: usize,
    /// Primitive index within the parent mesh (material-group order).
    pub primitive_index: usize,
    pub material_name: String,
    /// Index into `ufbx::Scene::materials` / loader material lists (`None` = default).
    pub material_index: Option<usize>,
    pub geometry_to_node: Mat4,
    /// Number of morph targets on this mesh (0 if none).
    pub morph_target_count: usize,
    /// Default blend channel weights (length == morph_target_count).
    pub morph_weights: Vec<f32>,
}

/// Optional FBX custom-property extras (glTF-style string blob).
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct FbxExtras {
    pub value: String,
}

/// Display name of an FBX mesh node (for tooling / queries).
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct FbxMeshName(pub String);

/// Display name of an FBX material.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct FbxMaterialName(pub String);

/// Scene display name (mirrors glTF scene name components).
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct FbxSceneName(pub String);

/// Scene-level extras blob.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct FbxSceneExtras {
    pub value: String,
}

/// Mesh-primitive extras blob.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct FbxMeshExtras {
    pub value: String,
}

/// Material extras blob on a mesh entity.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct FbxMaterialExtras {
    pub value: String,
}

// ============================================================================
// Main FBX Asset
// ============================================================================

/// Representation of a loaded FBX file.
#[derive(Asset, Debug, TypePath)]
pub struct Fbx {
    pub scenes: Vec<Handle<WorldAsset>>,
    pub named_scenes: HashMap<Box<str>, Handle<WorldAsset>>,
    /// Parent FBX meshes (material-split groups), labeled `Mesh{N}`.
    pub meshes: Vec<Handle<FbxMesh>>,
    pub named_meshes: HashMap<Box<str>, Handle<FbxMesh>>,
    /// Flat list of Bevy [`Mesh`] handles extracted from each [`FbxPrimitive::mesh`].
    pub primitive_meshes: Vec<Handle<Mesh>>,
    /// Parent FBX materials (labeled `Material{N}`); use [`.material`](FbxMaterial::material) for [`StandardMaterial`].
    pub materials: Vec<Handle<FbxMaterial>>,
    pub named_materials: HashMap<Box<str>, Handle<FbxMaterial>>,
    pub nodes: Vec<Handle<FbxNode>>,
    pub named_nodes: HashMap<Box<str>, Handle<FbxNode>>,
    pub skins: Vec<Handle<FbxSkin>>,
    pub named_skins: HashMap<Box<str>, Handle<FbxSkin>>,
    #[cfg(feature = "animation")]
    pub animations: Vec<Handle<AnimationClip>>,
    /// Take / anim-stack name → clip (e.g. `"Take 001"`). Prefer this over a
    /// second asset label — clips are only labeled `Animation{N}`.
    #[cfg(feature = "animation")]
    pub named_animations: HashMap<Box<str>, Handle<AnimationClip>>,
    pub default_scene: Option<Handle<WorldAsset>>,
    pub axis_system: FbxAxisSystem,
    pub unit_scale: f32,
    pub metadata: FbxMeta,
    /// Raw FBX bytes when [`crate::FbxLoaderSettings::include_source`] is set.
    pub source: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_name_joins_mesh_and_material() {
        assert_eq!(FbxPrimitive::name_for("Cube", Some("Wood")), "Cube.Wood");
    }

    #[test]
    fn primitive_name_without_material_falls_back_to_mesh() {
        assert_eq!(FbxPrimitive::name_for("Cube", None), "Cube");
        assert_eq!(FbxPrimitive::name_for("Cube", Some("")), "Cube");
    }

    #[test]
    fn primitive_name_unnamed_mesh_falls_back_to_mesh() {
        assert_eq!(FbxPrimitive::name_for("", Some("Wood")), "Mesh.Wood");
        assert_eq!(FbxPrimitive::name_for("", None), "Mesh");
    }
}
