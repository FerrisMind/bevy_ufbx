//! Regression: Maya Lambert opacity must not zero mesh alpha.

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::camera::primitives::MeshAabb;
use bevy::material::AlphaMode;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxPlugin};

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
        .init_asset::<AnimationClip>()
        .init_asset::<WorldAsset>();
    app
}

fn wait_for_load(app: &mut App, handle: &Handle<Fbx>, max_frames: usize) -> bool {
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

/// `blend_shape_cube.fbx` uses Maya `lambert1` with TransparencyFactor=1 and
/// black TransparentColor (opaque). Treating factor alone as `1 - factor`
/// used to force alpha=0 / Blend and hide the morph_fbx cube.
#[test]
fn blend_shape_cube_lambert_stays_opaque() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("blend_shape_cube.fbx")
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "blend_shape_cube.fbx failed to load"
    );

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx");
        fbx.default_scene.clone().expect("scene")
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds.get(&scene).expect("Scene0");
    let w = &world_asset.world;

    let mesh3d_count = w
        .iter_entities()
        .filter(|e| w.get::<Mesh3d>(e.id()).is_some())
        .count();
    assert_eq!(mesh3d_count, 1, "expected one Mesh3d primitive");

    let meshes = app.world().resource::<Assets<Mesh>>();
    let materials = app.world().resource::<Assets<StandardMaterial>>();

    let mut checked_material = false;
    for e in w.iter_entities() {
        let id = e.id();
        let Some(Mesh3d(mesh_handle)) = w.get::<Mesh3d>(id) else {
            continue;
        };
        let mesh = meshes.get(mesh_handle).expect("mesh asset");
        let aabb = mesh.compute_aabb().expect("mesh AABB");
        assert!(
            aabb.half_extents.max_element() > 1e-6,
            "degenerate mesh AABB: {aabb:?}"
        );

        let MeshMaterial3d(mat_handle) = w
            .get::<MeshMaterial3d<StandardMaterial>>(id)
            .expect("MeshMaterial3d");
        let mat = materials.get(mat_handle).expect("material asset");
        assert!(
            mat.base_color.alpha() > 0.99,
            "Lambert cube must stay opaque; got alpha={}",
            mat.base_color.alpha()
        );
        assert!(
            matches!(mat.alpha_mode, AlphaMode::Opaque),
            "expected Opaque alpha_mode, got {:?}",
            mat.alpha_mode
        );
        checked_material = true;
    }
    assert!(checked_material, "no MeshMaterial3d found on scene");
}

/// Raw ufbx fixture properties that previously triggered the bad fallback.
#[test]
fn blend_shape_cube_ufbx_opacity_not_authored() {
    let scene =
        ufbx::load_file("assets/blend_shape_cube.fbx", ufbx::LoadOpts::default()).expect("ufbx");
    let mat = scene
        .materials
        .as_ref()
        .iter()
        .find(|m| m.element.element_id != 0)
        .expect("lambert material");
    assert!(
        !mat.pbr.opacity.has_value,
        "ufbx must not mark pbr.opacity present for opaque Maya Lambert"
    );
    assert!(
        !mat.features.opacity.enabled,
        "features.opacity should be disabled for opaque Maya Lambert"
    );
    assert!(mat.fbx.transparency_factor.has_value);
    assert!((mat.fbx.transparency_factor.value_vec4.x - 1.0).abs() < 1e-5);
    let tc = mat.fbx.transparency_color.value_vec4;
    assert!(tc.x.abs() < 1e-5 && tc.y.abs() < 1e-5 && tc.z.abs() < 1e-5);
}
