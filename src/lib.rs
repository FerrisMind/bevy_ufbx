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
//! Bevy `AnimationClip`s via `ufbx::bake_anim`.

use std::sync::{Arc, Mutex, OnceLock};

use bevy::asset::AssetApp;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats, ImageSamplerDescriptor};
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
///
/// The plugin-level default image sampler is available as the
/// [`DefaultFbxImageSampler`] resource; see its documentation for the
/// precedence rules between plugin, per-load and per-texture settings.
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
            .init_resource::<DefaultFbxImageSampler>()
            .init_resource::<FbxCompressedImageFormatSupport>()
            .register_type::<FbxExtras>()
            .register_type::<FbxMeshName>()
            .register_type::<FbxMaterialName>()
            .register_type::<FbxSceneName>()
            .register_type::<FbxSceneExtras>()
            .register_type::<FbxMeshExtras>()
            .register_type::<FbxMaterialExtras>();

        // Register the loader in `build`, not `finish` (where `bevy_gltf`
        // registers `GltfLoader`): `FbxLoader` needs no finish-time data — the
        // compressed-format support is read lazily via
        // [`cached_supported_compressed_formats`], which [`Self::finish`] fills
        // once the render device is known. Registering here keeps FBX loading
        // working in `App::update`-only harnesses (tests, custom runners) that
        // never call `App::finish`, with no behavioral difference for apps.
        let default_sampler = app
            .world()
            .resource::<DefaultFbxImageSampler>()
            .get_internal();
        app.register_asset_loader(FbxLoader { default_sampler });
    }

    fn finish(&self, app: &mut App) {
        // Mirrors `bevy_gltf`'s `GltfPlugin::finish`: read the compressed-format
        // support the `RenderPlugin` derived from the actual render device and
        // expose it to texture processing.
        let supported_compressed_formats = if let Some(resource) =
            app.world().get_resource::<CompressedImageFormatSupport>()
        {
            resource.0
        } else {
            warn!(
                "CompressedImageFormatSupport resource not found. It should either be initialized in finish() of \
            RenderPlugin, or manually if not using the RenderPlugin or the WGPU backend."
            );
            CompressedImageFormats::NONE
        };

        app.insert_resource(FbxCompressedImageFormatSupport(
            supported_compressed_formats,
        ));
        // Load-time code (the asset loader) has no ECS access; cache the value
        // for `cached_supported_compressed_formats`.
        let _ = SUPPORTED_COMPRESSED_FORMATS.set(supported_compressed_formats);
    }
}

/// Stores the plugin-level default [`ImageSamplerDescriptor`] for FBX textures.
///
/// Inserted by [`FbxPlugin`]. The internal `Arc<Mutex<..>>` is shared with the
/// registered [`FbxLoader`], so mutating it through [`Self::set`] (or through
/// [`Self::get_internal`]) affects subsequent loads without re-registering the
/// loader.
///
/// # Sampler precedence
///
/// Mirrors `bevy_gltf`'s `DefaultGltfImageSampler` semantics, adapted to the
/// existing [`FbxLoaderSettings`] fields:
///
/// 1. [`FbxLoaderSettings::override_sampler`](crate::FbxLoaderSettings::override_sampler):
///    when `Some`, it is used for every texture of the load as-is and all
///    other sampler sources are ignored (FBX wrap modes included).
/// 2. [`FbxLoaderSettings::default_sampler`](crate::FbxLoaderSettings::default_sampler):
///    the per-load base sampler; FBX wrap modes are applied on top. It wins
///    over this resource only when it differs from
///    [`ImageSamplerDescriptor::default()`] (the field's serde default), which
///    is how "not set by the user" is detected — the glTF-style
///    `Option<ImageSamplerDescriptor>` would be a breaking change.
/// 3. This resource: the plugin-level default, used as the base when the
///    per-load default is left at its default value. Its built-in value is
///    [`ImageSamplerDescriptor::linear()`] — mirroring `bevy_gltf`'s
///    `GltfPlugin::default()` — not `ImageSamplerDescriptor::default()`
///    (Nearest filters).
///
/// The resolved base is what [`FbxLoaderSettings::default_sampler`] would
/// otherwise be; `resolve_default_sampler` performs steps 2-3.
///
/// Unlike a per-load setting, changes made through [`Self::set`] apply to all
/// subsequent loads without reloading plugin state; assets already loaded keep
/// their built samplers.
#[derive(Resource)]
pub struct DefaultFbxImageSampler(Arc<Mutex<ImageSamplerDescriptor>>);

impl Default for DefaultFbxImageSampler {
    fn default() -> Self {
        // glTF parity: `GltfPlugin::default()` registers
        // `ImageSamplerDescriptor::linear()`, not the Nearest-filter
        // `ImageSamplerDescriptor::default()`.
        Self::new(&ImageSamplerDescriptor::linear())
    }
}

impl DefaultFbxImageSampler {
    /// Creates a new [`DefaultFbxImageSampler`].
    pub fn new(descriptor: &ImageSamplerDescriptor) -> Self {
        Self(Arc::new(Mutex::new(descriptor.clone())))
    }

    /// Returns the current default [`ImageSamplerDescriptor`].
    pub fn get(&self) -> ImageSamplerDescriptor {
        self.0.lock().unwrap().clone()
    }

    /// Makes a clone of internal [`Arc`] pointer.
    ///
    /// Intended only to be used by code with no access to ECS.
    pub fn get_internal(&self) -> Arc<Mutex<ImageSamplerDescriptor>> {
        self.0.clone()
    }

    /// Replaces default [`ImageSamplerDescriptor`].
    ///
    /// Doesn't apply to samplers already built on top of it, i.e. `FbxLoader`'s
    /// output. Assets need to manually be reloaded.
    pub fn set(&self, descriptor: &ImageSamplerDescriptor) {
        *self.0.lock().unwrap() = descriptor.clone();
    }
}

/// Resolves the effective base sampler of a load from the plugin-level default
/// ([`DefaultFbxImageSampler::get_internal`], as held by the loader) and the
/// per-load [`FbxLoaderSettings::default_sampler`].
///
/// See the [`DefaultFbxImageSampler`] documentation for the full precedence
/// rules. The per-load value wins only when it was explicitly set, i.e. when
/// it differs from [`ImageSamplerDescriptor::default()`]; otherwise the
/// plugin-level default is used.
// Consumed by the loader wiring (FbxLoader side); see
// .swarm/results/wave1-plugin.md for the contract.
pub(crate) fn resolve_default_sampler(
    plugin_default: &Arc<Mutex<ImageSamplerDescriptor>>,
    per_load_default: &ImageSamplerDescriptor,
) -> ImageSamplerDescriptor {
    if *per_load_default == ImageSamplerDescriptor::default() {
        plugin_default.lock().unwrap().clone()
    } else {
        per_load_default.clone()
    }
}

/// Which compressed image formats texture processing may select, read from the
/// [`CompressedImageFormatSupport`] resource (initialized from the render
/// device by `RenderPlugin::finish`) in [`FbxPlugin::finish`] — mirroring
/// `bevy_gltf`'s `GltfPlugin::finish`.
///
/// Initialized to [`CompressedImageFormats::NONE`] by [`FbxPlugin::build`] and
/// replaced by the device-derived value in [`FbxPlugin::finish`]; the fallback
/// stays `NONE` (with a warning) when the `RenderPlugin` is not used.
///
/// Load-time code has no ECS access and should prefer
/// `cached_supported_compressed_formats`.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FbxCompressedImageFormatSupport(pub CompressedImageFormats);

/// Cached copy of [`FbxCompressedImageFormatSupport`] for load-time code (the
/// asset loader) that has no ECS access.
///
/// Set once by the first [`FbxPlugin::finish`] in the process; until then (and
/// when no `FbxPlugin` finished) it reports [`CompressedImageFormats::NONE`].
static SUPPORTED_COMPRESSED_FORMATS: OnceLock<CompressedImageFormats> = OnceLock::new();

/// Returns the compressed image formats recorded by [`FbxPlugin::finish`].
///
/// Backed by a process-wide cache set once by the first finished plugin; ECS
/// code should read the [`FbxCompressedImageFormatSupport`] resource instead.
// Consumed by texture processing (material/texture.rs), which the asset loader
// calls without ECS access.
pub(crate) fn cached_supported_compressed_formats() -> CompressedImageFormats {
    *SUPPORTED_COMPRESSED_FORMATS
        .get()
        .unwrap_or(&CompressedImageFormats::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImageAddressMode;

    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin {
                file_path: "assets".into(),
                ..default()
            })
            .add_plugins(FbxPlugin);
        app
    }

    #[test]
    fn default_fbx_image_sampler_resource_roundtrip() {
        let mut descriptor = ImageSamplerDescriptor::default();
        descriptor.address_mode_u = ImageAddressMode::Repeat;
        let resource = DefaultFbxImageSampler::new(&descriptor);
        assert_eq!(resource.get(), descriptor);

        let mut changed = ImageSamplerDescriptor::default();
        changed.address_mode_v = ImageAddressMode::MirrorRepeat;
        resource.set(&changed);
        assert_eq!(resource.get(), changed);
    }

    #[test]
    fn default_fbx_image_sampler_internal_arc_is_shared() {
        let resource = DefaultFbxImageSampler::default();
        let arc = resource.get_internal();

        let mut changed = ImageSamplerDescriptor::default();
        changed.anisotropy_clamp = 4;
        *arc.lock().unwrap() = changed.clone();
        assert_eq!(resource.get(), changed);
    }

    #[test]
    fn plugin_build_inserts_default_sampler_resource() {
        let app = headless_app();
        // Built-in default is `linear()`, matching `GltfPlugin::default()`
        // (glTF parity); the per-load sentinel stays `default()`.
        assert_eq!(
            app.world().resource::<DefaultFbxImageSampler>().get(),
            ImageSamplerDescriptor::linear()
        );
        assert_eq!(
            app.world().resource::<FbxCompressedImageFormatSupport>().0,
            CompressedImageFormats::NONE
        );
    }

    #[test]
    fn resolve_default_sampler_per_load_wins_when_explicitly_set() {
        let resource = DefaultFbxImageSampler::new(&ImageSamplerDescriptor::default());
        let mut per_load = ImageSamplerDescriptor::default();
        per_load.address_mode_u = ImageAddressMode::Repeat;
        assert_eq!(
            resolve_default_sampler(&resource.get_internal(), &per_load),
            per_load
        );
    }

    #[test]
    fn resolve_default_sampler_falls_back_to_plugin_default_when_per_load_unset() {
        let mut plugin_default = ImageSamplerDescriptor::default();
        plugin_default.address_mode_u = ImageAddressMode::MirrorRepeat;
        let resource = DefaultFbxImageSampler::new(&plugin_default);
        assert_eq!(
            resolve_default_sampler(&resource.get_internal(), &ImageSamplerDescriptor::default()),
            plugin_default
        );
    }

    #[test]
    fn plugin_finish_reads_compressed_image_format_support() {
        let mut app = headless_app();
        // Inserted manually, as when not using the RenderPlugin or WGPU backend.
        app.insert_resource(CompressedImageFormatSupport(CompressedImageFormats::BC));
        FbxPlugin.finish(&mut app);
        assert_eq!(
            app.world().resource::<FbxCompressedImageFormatSupport>().0,
            CompressedImageFormats::BC
        );
        assert_eq!(
            cached_supported_compressed_formats(),
            CompressedImageFormats::BC
        );
    }
}
