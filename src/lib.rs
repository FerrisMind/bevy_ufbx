#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![forbid(unsafe_code)]
#![doc(
    html_logo_url = "https://bevyengine.org/assets/icon.png",
    html_favicon_url = "https://bevyengine.org/assets/icon.png"
)]

//! Loader for FBX scenes using [`ufbx`](https://github.com/ufbx/ufbx-rust).
//!
//! Supports meshes, PBR materials, skinned meshes, hierarchical scenes,
//! lights/cameras, and (with the default `animation` feature) baked
//! [`AnimationClip`](bevy::animation::AnimationClip)s via `ufbx::bake_anim`.

use bevy::asset::AssetApp;
use bevy::prelude::*;

#[cfg(feature = "animation")]
pub mod animation;
pub mod error;
pub mod label;
pub mod loader;
pub mod material;
pub mod mesh;
pub mod names;
pub mod node;
pub mod scene;
pub mod types;
pub mod utils;

pub use error::FbxError;
pub use label::FbxAssetLabel;
pub use loader::{FbxLoader, FbxLoaderSettings, FbxSkinnedMeshBoundsPolicy, FbxSpaceConversion};
pub use types::*;

pub mod prelude {
    //! Commonly used items.
    pub use crate::{
        Fbx, FbxAssetLabel, FbxLoaderSettings, FbxMaterial, FbxMesh, FbxNode, FbxPlugin, FbxSkin,
        Skeleton,
    };
}

/// Plugin adding the FBX loader to an [`App`].
#[derive(Default)]
pub struct FbxPlugin;

impl Plugin for FbxPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<Fbx>()
            .init_asset::<FbxMesh>()
            .init_asset::<FbxMaterial>()
            .init_asset::<FbxNode>()
            .init_asset::<FbxSkin>()
            .init_asset::<Skeleton>()
            .register_type::<FbxExtras>()
            .register_type::<FbxMeshName>()
            .register_type::<FbxMaterialName>()
            .register_type::<FbxSceneName>()
            .register_type::<FbxSceneExtras>()
            .register_type::<FbxMeshExtras>()
            .register_type::<FbxMaterialExtras>()
            .register_asset_loader(FbxLoader);
    }
}
