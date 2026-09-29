//! Static mesh hierarchy + materials + unit conversion.
//!
//! ```sh
//! cargo run --example static_mesh_fbx
//! ```
//!
//! Prefers Blender Suzanne (`AdjustTransforms`); falls back to `cube.fbx`, then
//! `maya_cube_7400_binary.fbx` (cm fixtures get a 100x demo scale).
//! Look for: recognizable mesh spinning with materials, sensible metre framing.

use std::f32::consts::PI;
use std::path::Path;

use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const SUZANNE: &str = "blender_282_suzanne_7400_binary.fbx";
const CUBE: &str = "cube.fbx";
const MAYA_CUBE: &str = "maya_cube_7400_binary.fbx";
const CM_DEMO_SCALE: f32 = 100.0;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — static_mesh_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, rotate)
        .run();
}

#[derive(Component)]
struct Spinning;

fn pick_fbx() -> (&'static str, f32) {
    let assets = Path::new("assets");
    if assets.join(SUZANNE).is_file() {
        (SUZANNE, 1.0)
    } else if assets.join(CUBE).is_file() {
        (CUBE, CM_DEMO_SCALE)
    } else {
        (MAYA_CUBE, CM_DEMO_SCALE)
    }
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (fbx_path, scale) = pick_fbx();

    commands.spawn((
        WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(fbx_path))),
        Transform::from_scale(Vec3::splat(scale)),
        Spinning,
    ));

    let cam = if scale > 10.0 {
        Vec3::new(2.5, 2.0, 4.0)
    } else {
        Vec3::new(3.0, 2.2, 4.5)
    };
    commands.spawn((
        Camera3d::default(),
        Transform::from_translation(cam).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.28, 0.4, 0.32))),
        Transform::from_xyz(0.0, -1.1, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.)),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 20.0,
            maximum_distance: 40.0,
            ..default()
        }
        .build(),
    ));

    commands.spawn((
        Text::new(format!(
            "static_mesh_fbx — {fbx_path}\n\
             Look for: hierarchy + materials + units (spinning static mesh).\n\
             Blender files use AdjustTransforms; Maya/cm use ModifyGeometry + demo scale."
        )),
        TextFont::from_font_size(18.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
