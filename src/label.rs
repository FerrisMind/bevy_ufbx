//! Labels that can be used to load part of an FBX asset
use bevy::asset::AssetPath;

/// Labels that can be used to load part of an FBX asset
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbxAssetLabel {
    /// `Scene{}`: FBX Scene as a Bevy [`WorldAsset`](bevy::world_serialization::WorldAsset)
    Scene(usize),
    /// `Mesh{}`: FBX Mesh as a Bevy [`Mesh`](bevy::render::mesh::Mesh)
    Mesh(usize),
    /// `Material{}`: FBX material as a Bevy [`StandardMaterial`](bevy::pbr::StandardMaterial)
    Material(usize),
    /// `Animation{}`: FBX animation as a Bevy [`AnimationClip`](bevy::animation::AnimationClip)
    Animation(usize),
    /// `Skeleton{}`: FBX skeleton for skeletal animation
    Skeleton(usize),
    /// `Node{}`: Individual FBX node in the scene hierarchy
    Node(usize),
    /// `Skin{}`: FBX skin for skeletal animation
    Skin(usize),
    /// `Skin{}/InverseBindMatrices`: inverse bind pose matrices for a skin
    InverseBindMatrices(usize),
    /// `Light{}`: FBX light definition
    Light(usize),
    /// `Camera{}`: FBX camera definition
    Camera(usize),
    /// `Texture{}`: FBX texture reference
    Texture(usize),
    /// `DefaultScene`: Main scene with all objects
    DefaultScene,
    /// `DefaultMaterial`: Fallback material used when no material is present
    DefaultMaterial,
    /// `RootNode`: Root node of the scene hierarchy
    RootNode,
}

impl core::fmt::Display for FbxAssetLabel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FbxAssetLabel::Scene(index) => write!(f, "Scene{index}"),
            FbxAssetLabel::Mesh(index) => write!(f, "Mesh{index}"),
            FbxAssetLabel::Material(index) => write!(f, "Material{index}"),
            FbxAssetLabel::Animation(index) => write!(f, "Animation{index}"),
            FbxAssetLabel::Skeleton(index) => write!(f, "Skeleton{index}"),
            FbxAssetLabel::Node(index) => write!(f, "Node{index}"),
            FbxAssetLabel::Skin(index) => write!(f, "Skin{index}"),
            FbxAssetLabel::InverseBindMatrices(index) => {
                write!(f, "Skin{index}/InverseBindMatrices")
            }
            FbxAssetLabel::Light(index) => write!(f, "Light{index}"),
            FbxAssetLabel::Camera(index) => write!(f, "Camera{index}"),
            FbxAssetLabel::Texture(index) => write!(f, "Texture{index}"),
            FbxAssetLabel::DefaultScene => f.write_str("DefaultScene"),
            FbxAssetLabel::DefaultMaterial => f.write_str("DefaultMaterial"),
            FbxAssetLabel::RootNode => f.write_str("RootNode"),
        }
    }
}

impl FbxAssetLabel {
    /// Add this label to an asset path
    pub fn from_asset(&self, path: impl Into<AssetPath<'static>>) -> AssetPath<'static> {
        path.into().with_label(self.to_string())
    }
}
