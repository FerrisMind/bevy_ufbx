//! Smoke: primary demo assets load Scene0 with ≥1 Mesh3d.

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::image::{Image, ImageSamplerDescriptor};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxLoaderSettings, FbxMaterial, FbxMesh, FbxPlugin};

fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: "assets".into(),
            ..default()
        })
        .add_plugins(FbxPlugin)
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .init_asset::<AnimationClip>()
        .init_asset::<SkinnedMeshInverseBindposes>()
        .init_asset::<WorldAsset>();
    app
}

fn wait_for_load<A: Asset>(app: &mut App, handle: &Handle<A>, max_frames: usize) -> bool {
    for _ in 0..max_frames {
        app.update();
        let server = app.world().resource::<AssetServer>();
        match server.load_state(handle) {
            LoadState::Loaded => return true,
            LoadState::Failed(_) => return false,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    false
}

fn assert_scene0_has_mesh3d(app: &mut App, path: &'static str) {
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(app, &fbx_handle, 800),
        "{path} failed to load"
    );

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs
            .get(&fbx_handle)
            .unwrap_or_else(|| panic!("{path}: missing Fbx"));
        fbx.default_scene
            .clone()
            .unwrap_or_else(|| panic!("{path}: missing Scene0"))
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds
        .get(&scene)
        .unwrap_or_else(|| panic!("{path}: Scene0 WorldAsset missing"));
    let count = world_asset
        .world
        .iter_entities()
        .filter(|e| world_asset.world.get::<Mesh3d>(e.id()).is_some())
        .count();
    assert!(
        count >= 1,
        "{path}: expected ≥1 Mesh3d in Scene0, got {count}"
    );
}

#[test]
fn smoke_blend_shape_cube_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "blend_shape_cube.fbx");
}

#[test]
fn smoke_cube_anim_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "cube_anim.fbx");
}

#[test]
fn smoke_nurbs_saddle_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "nurbs_saddle.fbx");
}

#[test]
fn smoke_cube_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "cube.fbx");
}

#[test]
fn smoke_rigged_triangle_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "rigged_triangle.fbx");
}

#[test]
fn smoke_sausage_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "blender_279_sausage_7400_binary.fbx");
}

#[test]
fn smoke_suzanne_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "blender_282_suzanne_7400_binary.fbx");
}

#[test]
fn smoke_internal_textures_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(
        &mut headless_app(),
        "blender_279_internal_textures_7400_binary.fbx",
    );
}

#[test]
fn smoke_nested_meshes_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(
        &mut headless_app(),
        "blender_279_nested_meshes_7400_binary.fbx",
    );
}

#[test]
fn smoke_suzanne_multimaterial_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(
        &mut headless_app(),
        "blender_suzanne_multimaterial_7400_binary.fbx",
    );
}

/// Multimaterial Suzanne must material-split into multiple Mesh3d primitives.
#[test]
fn smoke_suzanne_multimaterial_has_multiple_mesh3d() {
    let mut app = headless_app();
    let path = "blender_suzanne_multimaterial_7400_binary.fbx";
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 800),
        "{path} failed to load"
    );

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs
            .get(&fbx_handle)
            .unwrap_or_else(|| panic!("{path}: missing Fbx"));
        assert!(
            fbx.primitive_meshes.len() >= 2,
            "{path}: expected ≥2 primitive meshes, got {}",
            fbx.primitive_meshes.len()
        );
        assert!(
            fbx.materials.len() >= 2,
            "{path}: expected ≥2 materials, got {}",
            fbx.materials.len()
        );
        fbx.default_scene
            .clone()
            .unwrap_or_else(|| panic!("{path}: missing Scene0"))
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds
        .get(&scene)
        .unwrap_or_else(|| panic!("{path}: Scene0 WorldAsset missing"));
    let count = world_asset
        .world
        .iter_entities()
        .filter(|e| world_asset.world.get::<Mesh3d>(e.id()).is_some())
        .count();
    assert!(
        count >= 2,
        "{path}: expected ≥2 Mesh3d (material-split), got {count}"
    );
}

/// Nested meshes fixture: multiple mesh nodes under one Scene0.
#[test]
fn smoke_nested_meshes_has_multiple_mesh3d() {
    let mut app = headless_app();
    let path = "blender_279_nested_meshes_7400_binary.fbx";
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 800),
        "{path} failed to load"
    );

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs
            .get(&fbx_handle)
            .unwrap_or_else(|| panic!("{path}: missing Fbx"));
        assert!(
            fbx.meshes.len() >= 2,
            "{path}: expected ≥2 FbxMesh containers, got {}",
            fbx.meshes.len()
        );
        fbx.default_scene
            .clone()
            .unwrap_or_else(|| panic!("{path}: missing Scene0"))
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds
        .get(&scene)
        .unwrap_or_else(|| panic!("{path}: Scene0 WorldAsset missing"));
    let count = world_asset
        .world
        .iter_entities()
        .filter(|e| world_asset.world.get::<Mesh3d>(e.id()).is_some())
        .count();
    assert!(
        count >= 2,
        "{path}: expected ≥2 Mesh3d (nested hierarchy), got {count}"
    );
}

/// Camera/light fixture has no geometry — still must load Scene0 successfully.
#[test]
fn smoke_maya_camera_light_loads_without_mesh3d() {
    let mut app = headless_app();
    let path = "maya_camera_light_axes_y_up_6100_binary.fbx";
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 800),
        "{path} failed to load"
    );
    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(&fbx_handle).expect("Fbx");
    assert!(fbx.default_scene.is_some(), "expected Scene0");
    assert!(
        fbx.primitive_meshes.is_empty(),
        "camera/light fixture should have no mesh primitives"
    );
}

#[test]
fn smoke_zbrush_vertex_color_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(&mut headless_app(), "zbrush_vertex_color_7500_ascii.fbx");
}

/// ZBrush fixture must carry `Mesh::ATTRIBUTE_COLOR` on at least one primitive.
#[test]
fn smoke_zbrush_vertex_color_has_attribute_color() {
    let mut app = headless_app();
    let path = "zbrush_vertex_color_7500_ascii.fbx";
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 800),
        "{path} failed to load"
    );

    let primitive_handles = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs
            .get(&fbx_handle)
            .unwrap_or_else(|| panic!("{path}: missing Fbx"));
        fbx.primitive_meshes.clone()
    };
    let meshes = app.world().resource::<Assets<Mesh>>();
    let with_color = primitive_handles
        .iter()
        .filter(|h| {
            meshes
                .get(*h)
                .is_some_and(|m| m.attribute(Mesh::ATTRIBUTE_COLOR).is_some())
        })
        .count();
    assert!(
        with_color >= 1,
        "{path}: expected ≥1 primitive with ATTRIBUTE_COLOR, got {with_color}"
    );
}

#[test]
fn smoke_mirrored_normals_scene_has_mesh3d() {
    assert_scene0_has_mesh3d(
        &mut headless_app(),
        "blender_340_mirrored_normals_7400_binary.fbx",
    );
}

/// Neg-scale fixture must spawn at least one Mesh3d using Front-cull (inverted twin).
#[test]
fn smoke_mirrored_normals_uses_inverted_cull() {
    use bevy::render::render_resource::Face;

    let mut app = headless_app();
    let path = "blender_340_mirrored_normals_7400_binary.fbx";
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 800),
        "{path} failed to load"
    );

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs
            .get(&fbx_handle)
            .unwrap_or_else(|| panic!("{path}: missing Fbx"));
        fbx.default_scene
            .clone()
            .unwrap_or_else(|| panic!("{path}: missing Scene0"))
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds
        .get(&scene)
        .unwrap_or_else(|| panic!("{path}: Scene0 WorldAsset missing"));
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let front_cull = world_asset
        .world
        .iter_entities()
        .filter(|e| {
            world_asset
                .world
                .get::<MeshMaterial3d<StandardMaterial>>(e.id())
                .and_then(|mm| materials.get(&mm.0))
                .is_some_and(|mat| mat.cull_mode == Some(Face::Front))
        })
        .count();
    assert!(
        front_cull >= 1,
        "{path}: expected ≥1 Mesh3d with Front cull (inverted), got {front_cull}"
    );
}

/// `materials_pbr_fbx` fixture: the example pairs `Material{N}` label loads with
/// an ufbx probe by dense index — both `Material0` and `Material0/Standard` must
/// resolve, and the headline claim (specular_factor 0.25 → reflectance 0.125)
/// must hold on the loaded `StandardMaterial`.
#[test]
fn smoke_materials_pbr_material_labels_resolve() {
    let mut app = headless_app();
    let path = "blender_279_internal_textures_7400_binary.fbx";
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 800),
        "{path} failed to load"
    );

    let material: Handle<FbxMaterial> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FbxAssetLabel::Material(0).from_asset(path))
    };
    let standard: Handle<StandardMaterial> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FbxAssetLabel::MaterialStandard(0).from_asset(path))
    };
    assert!(
        wait_for_load(&mut app, &material, 400),
        "{path}: Material0 label failed to resolve"
    );
    assert!(
        wait_for_load(&mut app, &standard, 400),
        "{path}: Material0/Standard label failed to resolve"
    );

    let reflectance = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&standard)
        .unwrap_or_else(|| panic!("{path}: Material0/Standard missing"))
        .reflectance;
    assert_eq!(
        reflectance, 0.125,
        "{path}: specular_factor 0.25 must map to reflectance 0.125 (materials_pbr_fbx demo)"
    );
}

/// `sampler_settings_fbx` tier-2 flow: a Scene0 load carrying a per-load
/// `FbxLoaderSettings::default_sampler` (≠ default, so the loader treats it as
/// set) resolves and still spawns ≥1 Mesh3d.
#[test]
fn smoke_sampler_settings_perload_scene_loads() {
    let mut app = headless_app();
    let path = "blender_279_internal_textures_7400_binary.fbx";
    let scene: Handle<WorldAsset> = {
        let server = app.world().resource::<AssetServer>();
        server
            .load_builder()
            .with_settings(|settings: &mut FbxLoaderSettings| {
                settings.default_sampler = ImageSamplerDescriptor {
                    label: Some("smoke: default_sampler".to_string()),
                    ..ImageSamplerDescriptor::default()
                };
            })
            .load(FbxAssetLabel::Scene(0).from_asset(path))
    };
    assert!(
        wait_for_load(&mut app, &scene, 800),
        "{path}: Scene0 failed under per-load default_sampler settings"
    );

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds
        .get(&scene)
        .unwrap_or_else(|| panic!("{path}: Scene0 WorldAsset missing"));
    let count = world_asset
        .world
        .iter_entities()
        .filter(|e| world_asset.world.get::<Mesh3d>(e.id()).is_some())
        .count();
    assert!(
        count >= 1,
        "{path}: expected ≥1 Mesh3d in Scene0, got {count}"
    );
}

/// `dump_fbx` label-resolution section: the typed `Mesh{N}` / `Material{N}` /
/// `Animation{N}` loads it demonstrates all resolve for its default fixture
/// (Scene0 covered by `smoke_cube_anim_scene_has_mesh3d`; contract semantics for
/// these labels live in `tests/parity_contract.rs`).
#[test]
fn smoke_dump_fbx_label_loads_resolve() {
    let mut app = headless_app();
    let path = "cube_anim.fbx";
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(path)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 800),
        "{path} failed to load"
    );

    let mesh: Handle<FbxMesh> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FbxAssetLabel::Mesh(0).from_asset(path))
    };
    let material: Handle<FbxMaterial> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FbxAssetLabel::Material(0).from_asset(path))
    };
    let animation: Handle<AnimationClip> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FbxAssetLabel::Animation(0).from_asset(path))
    };
    assert!(
        wait_for_load(&mut app, &mesh, 400),
        "{path}: Mesh0 label failed to resolve"
    );
    assert!(
        wait_for_load(&mut app, &material, 400),
        "{path}: Material0 label failed to resolve"
    );
    assert!(
        wait_for_load(&mut app, &animation, 400),
        "{path}: Animation0 label failed to resolve"
    );
}
