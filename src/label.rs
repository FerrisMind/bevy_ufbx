//! Labels that can be used to load part of an FBX asset.
use bevy::asset::AssetPath;

/// Labels that can be used to load part of an FBX asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbxAssetLabel {
    /// `Scene{}`: FBX Scene as a Bevy [`WorldAsset`](bevy::world_serialization::WorldAsset)
    Scene(usize),
    /// `Mesh{}`: parent [`FbxMesh`](crate::FbxMesh) container (glTF-aligned).
    Mesh(usize),
    /// `Mesh{}/Primitive{}`: Bevy [`Mesh`](bevy::mesh::Mesh) for one
    /// material-split primitive of a parent FBX mesh (glTF-compatible path).
    Primitive {
        /// Index of the parent FBX mesh (one per mesh-bearing node / NURBS).
        mesh: usize,
        /// Index of this material-group primitive within that mesh.
        primitive: usize,
    },
    /// `Material{}`: FBX material as [`FbxMaterial`](crate::FbxMaterial)
    Material(usize),
    /// `Material{}/Standard`: Bevy [`StandardMaterial`](bevy::pbr::StandardMaterial)
    /// referenced by [`FbxMaterial::material`](crate::FbxMaterial::material)
    MaterialStandard(usize),
    /// `Material{} (inverted)`: cull-inverted [`StandardMaterial`](bevy::pbr::StandardMaterial)
    /// for negative-scale nodes (not wrapped in [`FbxMaterial`](crate::FbxMaterial))
    MaterialInverted(usize),
    /// `Animation{}`: FBX animation as a Bevy `AnimationClip` (`animation` feature)
    Animation(usize),
    /// `AnimationRest`: optional rest/bind pose clip (see [`crate::FbxLoaderSettings::generate_rest_animation`])
    AnimationRest,
    /// `Node{}`: Individual FBX node in the scene hierarchy
    Node(usize),
    /// `Skin{}`: FBX skin for skeletal animation
    Skin(usize),
    /// `Skin{}/InverseBindMatrices`: inverse bind pose matrices for a skin
    InverseBindMatrices(usize),
    /// `Texture{}`: embedded or referenced texture image
    Texture(usize),
    /// `DefaultMaterial`: Fallback material used when no material is present
    DefaultMaterial,
}

impl core::fmt::Display for FbxAssetLabel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FbxAssetLabel::Scene(index) => write!(f, "Scene{index}"),
            FbxAssetLabel::Mesh(index) => write!(f, "Mesh{index}"),
            FbxAssetLabel::Primitive { mesh, primitive } => {
                write!(f, "Mesh{mesh}/Primitive{primitive}")
            }
            FbxAssetLabel::Material(index) => write!(f, "Material{index}"),
            FbxAssetLabel::MaterialStandard(index) => write!(f, "Material{index}/Standard"),
            FbxAssetLabel::MaterialInverted(index) => write!(f, "Material{index} (inverted)"),
            FbxAssetLabel::Animation(index) => write!(f, "Animation{index}"),
            FbxAssetLabel::AnimationRest => f.write_str("AnimationRest"),
            FbxAssetLabel::Node(index) => write!(f, "Node{index}"),
            FbxAssetLabel::Skin(index) => write!(f, "Skin{index}"),
            FbxAssetLabel::InverseBindMatrices(index) => {
                write!(f, "Skin{index}/InverseBindMatrices")
            }
            FbxAssetLabel::Texture(index) => write!(f, "Texture{index}"),
            FbxAssetLabel::DefaultMaterial => f.write_str("DefaultMaterial"),
        }
    }
}

impl FbxAssetLabel {
    /// Add this label to an asset path.
    pub fn from_asset(&self, path: impl Into<AssetPath<'static>>) -> AssetPath<'static> {
        path.into().with_label(self.to_string())
    }
}
