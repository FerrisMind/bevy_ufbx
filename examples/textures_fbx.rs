//! Embedded FBX textures / wrap modes.
//!
//! ```sh
//! cargo run --example textures_fbx
//! ```
//!
//! Asset: `assets/blender_279_internal_textures_7400_binary.fbx`
//! Look for: textured cube (embedded image), not flat grey default.

use std::f32::consts::PI;
use std::path::Path;

use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const TEXTURES: &str = "blender_279_internal_textures_7400_binary.fbx";
/// Blender 2.79 fixture reports centimetre-ish extents after conversion — enlarge.
const DEMO_VISUAL_SCALE: f32 = 100.0;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — textures_fbx".into(),
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

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    assert!(
        Path::new("assets").join(TEXTURES).is_file(),
        "missing assets/{TEXTURES} — copy from ufbx/data"
    );

    commands.spawn((
        WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(TEXTURES))),
        Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
        Spinning,
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(2.5, 2.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.25, 0.28, 0.32))),
        Transform::from_xyz(0.0, -0.55, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.)),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 15.0,
            maximum_distance: 30.0,
            ..default()
        }
        .build(),
    ));

    commands.spawn((
        Text::new(
            "textures_fbx — blender_279_internal_textures_7400_binary.fbx\n\
             Look for: EMBEDDED base-color texture on the cube (not flat grey).\n\
             Proves Image labels + wrap/sampler mapping from FBX.",
        ),
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
