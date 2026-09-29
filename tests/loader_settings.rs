//! Tests for FBX loader settings.

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSamplerDescriptor;
use bevy_ufbx::{FbxLoaderSettings, FbxSkinnedMeshBoundsPolicy, FbxSpaceConversion};

#[test]
fn test_loader_settings_default() {
    let settings = FbxLoaderSettings::default();

    assert_eq!(settings.load_meshes, RenderAssetUsages::default());
    assert_eq!(settings.load_materials, RenderAssetUsages::default());
    assert!(settings.load_cameras);
    assert!(settings.load_lights);
    assert!(settings.load_animations);
    assert_eq!(settings.bake_fps, 30.0);
    assert!(!settings.generate_rest_animation);
    assert!(!settings.include_source);
    assert!(settings.convert_coordinates);
    assert_eq!(settings.space_conversion, FbxSpaceConversion::Auto);
    assert_eq!(
        settings.skinned_mesh_bounds_policy,
        FbxSkinnedMeshBoundsPolicy::Dynamic
    );
    assert_eq!(settings.default_sampler, ImageSamplerDescriptor::default());
    assert!(settings.override_sampler.is_none());
}

#[test]
fn test_loader_settings_custom() {
    let override_sampler = ImageSamplerDescriptor::nearest();
    let settings = FbxLoaderSettings {
        load_meshes: RenderAssetUsages::RENDER_WORLD,
        load_materials: RenderAssetUsages::MAIN_WORLD,
        load_cameras: false,
        load_lights: false,
        load_animations: false,
        bake_fps: 24.0,
        generate_rest_animation: true,
        include_source: true,
        convert_coordinates: false,
        space_conversion: FbxSpaceConversion::AdjustTransforms,
        skinned_mesh_bounds_policy: FbxSkinnedMeshBoundsPolicy::NoFrustumCulling,
        default_sampler: ImageSamplerDescriptor::linear(),
        override_sampler: Some(override_sampler.clone()),
    };

    assert!(!settings.convert_coordinates);
    assert_eq!(settings.bake_fps, 24.0);
    assert!(settings.generate_rest_animation);
    assert_eq!(
        settings.space_conversion,
        FbxSpaceConversion::AdjustTransforms
    );
    assert_eq!(
        settings.skinned_mesh_bounds_policy,
        FbxSkinnedMeshBoundsPolicy::NoFrustumCulling
    );
    assert_eq!(settings.default_sampler, ImageSamplerDescriptor::linear());
    assert_eq!(settings.override_sampler, Some(override_sampler));
}

#[test]
fn test_loader_settings_serialization() {
    let original = FbxLoaderSettings {
        load_meshes: RenderAssetUsages::RENDER_WORLD,
        load_materials: RenderAssetUsages::MAIN_WORLD,
        load_cameras: false,
        load_lights: true,
        load_animations: true,
        bake_fps: 60.0,
        generate_rest_animation: true,
        include_source: false,
        convert_coordinates: true,
        space_conversion: FbxSpaceConversion::TransformRoot,
        skinned_mesh_bounds_policy: FbxSkinnedMeshBoundsPolicy::BindPose,
        default_sampler: ImageSamplerDescriptor::nearest(),
        override_sampler: Some(ImageSamplerDescriptor::linear()),
    };

    let serialized = serde_json::to_string(&original).expect("Failed to serialize");
    let deserialized: FbxLoaderSettings =
        serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(deserialized.space_conversion, original.space_conversion);
    assert_eq!(deserialized.bake_fps, original.bake_fps);
    assert_eq!(
        deserialized.generate_rest_animation,
        original.generate_rest_animation
    );
    assert_eq!(
        deserialized.skinned_mesh_bounds_policy,
        original.skinned_mesh_bounds_policy
    );
    assert_eq!(deserialized.default_sampler, original.default_sampler);
    assert_eq!(deserialized.override_sampler, original.override_sampler);
}
