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

/// Material representation.
#[derive(Debug, Clone)]
pub struct FbxMaterial {
    pub name: String,
    pub base_color: Color,
    pub metallic: f32,
    pub roughness: f32,
    pub emission: Color,
    pub normal_scale: f32,
    pub alpha: f32,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    pub textures: HashMap<FbxTextureType, FbxTexture>,
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

/// FBX node with hierarchy.
#[derive(Asset, Debug, Clone, TypePath)]
pub struct FbxNode {
    pub index: usize,
    pub name: String,
    pub children: Vec<Handle<FbxNode>>,
    pub mesh: Option<Handle<Mesh>>,
    pub skin: Option<Handle<FbxSkin>>,
    pub transform: Transform,
    pub visible: bool,
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
    pub material_name: String,
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

// ============================================================================
// Main FBX Asset
// ============================================================================

/// Representation of a loaded FBX file.
#[derive(Asset, Debug, TypePath)]
pub struct Fbx {
    pub scenes: Vec<Handle<WorldAsset>>,
    pub named_scenes: HashMap<Box<str>, Handle<WorldAsset>>,
    pub meshes: Vec<Handle<Mesh>>,
    pub named_meshes: HashMap<Box<str>, Handle<Mesh>>,
    pub materials: Vec<Handle<StandardMaterial>>,
    pub named_materials: HashMap<Box<str>, Handle<StandardMaterial>>,
    pub nodes: Vec<Handle<FbxNode>>,
    pub named_nodes: HashMap<Box<str>, Handle<FbxNode>>,
    pub skins: Vec<Handle<FbxSkin>>,
    pub named_skins: HashMap<Box<str>, Handle<FbxSkin>>,
    #[cfg(feature = "animation")]
    pub animations: Vec<Handle<AnimationClip>>,
    #[cfg(feature = "animation")]
    pub named_animations: HashMap<Box<str>, Handle<AnimationClip>>,
    pub default_scene: Option<Handle<WorldAsset>>,
    pub axis_system: FbxAxisSystem,
    pub unit_scale: f32,
    pub metadata: FbxMeta,
    /// Raw FBX bytes when [`crate::FbxLoaderSettings::include_source`] is set.
    pub source: Option<Vec<u8>>,
}
