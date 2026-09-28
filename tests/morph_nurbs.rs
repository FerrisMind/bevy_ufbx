//! Integration tests for morph targets and NURBS tessellation.

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::mesh::morph::MorphWeights;
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

#[test]
fn blend_shape_cube_loads_morph_targets() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("blend_shape_cube.fbx")
    };

    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "blend_shape_cube.fbx failed to load"
    );

    let (mesh_handle, scene) = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx");
        assert!(!fbx.meshes.is_empty());
        (
            fbx.meshes[0].clone(),
            fbx.default_scene.clone().expect("scene"),
        )
    };

    let meshes = app.world().resource::<Assets<Mesh>>();
    let mesh = meshes.get(&mesh_handle).expect("Mesh0");
    assert!(
        mesh.morph_targets().is_some(),
        "expected morph target buffer on blend shape cube"
    );

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds.get(&scene).expect("Scene0");
    let has_morph_weights = world_asset
        .world
        .iter_entities()
        .any(|e| world_asset.world.get::<MorphWeights>(e.id()).is_some());
    assert!(
        has_morph_weights,
        "scene should contain MorphWeights on a blend-shape node"
    );
}

#[test]
fn nurbs_saddle_tessellates_to_mesh() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("nurbs_saddle.fbx")
    };

    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "nurbs_saddle.fbx failed to load"
    );

    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(&fbx_handle).expect("Fbx");
    assert!(
        !fbx.meshes.is_empty(),
        "NURBS surface should tessellate into at least one Mesh"
    );
}
