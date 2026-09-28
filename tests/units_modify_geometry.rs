//! Metre-native unit conversion (ModifyGeometry default).

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::prelude::*;
use bevy::render::mesh::VertexAttributeValues;
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
fn maya_cm_cube_bakes_into_metre_geometry() {
    let mut app = headless_app();
    let handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("blend_shape_cube.fbx")
    };
    assert!(
        wait_for_load(&mut app, &handle, 500),
        "blend_shape_cube.fbx failed to load"
    );

    let mesh_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&handle).expect("Fbx");
        // convert_coordinates reports Bevy metres
        assert!((fbx.unit_scale - 1.0).abs() < 1e-5);
        fbx.primitive_meshes[0].clone()
    };

    let meshes = app.world().resource::<Assets<Mesh>>();
    let mesh = meshes.get(&mesh_handle).expect("Mesh0");
    let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        panic!("missing positions");
    };
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for p in pos {
        let v = Vec3::from(*p);
        min = min.min(v);
        max = max.max(v);
    }
    let extent = max - min;
    // Maya 1-unit cube in cm → 0.01 m after ModifyGeometry (not ±1 with scale 0.01).
    assert!(
        (extent.x - 0.01).abs() < 1e-4 && (extent.y - 0.01).abs() < 1e-4,
        "expected ~1cm mesh extent after ModifyGeometry, got {extent:?}"
    );
}
