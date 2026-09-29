//! Loads and displays an FBX file.
//!
//! Pass a path (relative to the `assets/` directory) as the first argument,
//! or drop a `cube.fbx` into `assets/` and run without arguments:
//!
//! ```sh
//! cargo run --example load_fbx -- spider.fbx
//! ```
//!
//! The loaded model spins slowly so you can inspect it from all sides.
//! Set `FBX_SETTINGS=1` to run the same load through
//! `AssetServer::load_builder().with_settings(...)` with `FbxLoaderSettings`
//! (see the `use_settings` branch in `setup`). Sampler-tier settings
//! (`default_sampler` / `override_sampler`) are demonstrated in
//! `examples/sampler_settings_fbx.rs`.

use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use bevy_ufbx::{FbxLoaderSettings, FbxPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(FbxPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, rotate)
        .run();
}

// ── Components / resources ──────────────────────────────────────────────────

#[derive(Component)]
struct Spinning;

// ── Setup ───────────────────────────────────────────────────────────────────

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // ── Camera ──────────────────────────────────────────────────────────────
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(4.0, 4.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // ── Lighting ─────────────────────────────────────────────────────────────
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(
            EulerRot::XYZ,
            -45_f32.to_radians(),
            45_f32.to_radians(),
            0.0,
        )),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 200.0,
        affects_lightmapped_meshes: false,
    });

    // ── FBX path from CLI args (defaults to "cube.fbx") ──────────────────────
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cube.fbx".to_string());

    // --- Load Scene0: default settings, or FbxLoaderSettings when FBX_SETTINGS=1 ---
    // Current API: `load_builder().with_settings(...)` (the old
    // `load_with_settings` is deprecated). Sampler-tier settings live in
    // `examples/sampler_settings_fbx.rs`; this branch shows import-shape knobs.
    let use_settings = std::env::var_os("FBX_SETTINGS").is_some();
    let scene = if use_settings {
        asset_server
            .load_builder()
            .with_settings(|settings: &mut FbxLoaderSettings| {
                // Full knob list on `FbxLoaderSettings`: load_meshes /
                // load_materials / load_animations / bake_fps /
                // generate_rest_animation / include_source / convert_coordinates /
                // space_conversion / skinned_mesh_bounds_policy / default_sampler /
                // override_sampler …
                settings.load_cameras = false;
                settings.load_lights = false;
            })
            .load::<WorldAsset>(format!("{path}#Scene0"))
    } else {
        asset_server.load(format!("{path}#Scene0"))
    };
    commands.spawn((WorldAssetRoot(scene), Spinning));

    if use_settings {
        info!("Loading '{path}' with FbxLoaderSettings (load_cameras=false, load_lights=false) …");
    } else {
        info!("Loading '{path}' … (FBX_SETTINGS=1 to demo FbxLoaderSettings)");
    }
}

// ── Systems ──────────────────────────────────────────────────────────────────

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
