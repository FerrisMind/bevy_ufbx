//! Integration tests: bake_anim → AnimationClip labels and Fbx named map.

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxPlugin};

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
fn cube_anim_loads_animation_clip_label() {
    let mut app = headless_app();
    let clip_handle: Handle<AnimationClip> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FbxAssetLabel::Animation(0).from_asset("cube_anim.fbx"))
    };
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("cube_anim.fbx")
    };

    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "cube_anim.fbx failed to load"
    );

    let clips = app.world().resource::<Assets<AnimationClip>>();
    let clip = clips
        .get(&clip_handle)
        .expect("Animation0 label should resolve to an AnimationClip");
    assert!(
        clip.duration() > 0.0,
        "baked clip should have positive duration"
    );

    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
    assert!(
        !fbx.animations.is_empty(),
        "Fbx.animations should list baked clips"
    );
    assert!(
        !fbx.named_animations.is_empty(),
        "named_animations should map take names to clips"
    );
}

#[test]
fn cube_anim_scene_spawns_animation_player() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("cube_anim.fbx")
    };

    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "cube_anim.fbx failed to load"
    );

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        fbx.default_scene.as_ref().expect("default_scene").clone()
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds.get(&scene).expect("Scene0 WorldAsset");
    let has_player = world_asset
        .world
        .iter_entities()
        .any(|e| world_asset.world.get::<AnimationPlayer>(e.id()).is_some());
    assert!(
        has_player,
        "scene WorldAsset should contain an AnimationPlayer on an animation root"
    );
}
