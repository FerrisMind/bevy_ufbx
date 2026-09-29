//! NURBS surface tessellated to a triangle mesh.
//!
//! ```sh
//! cargo run --example nurbs_fbx
//! ```
//!
//! Asset: `assets/nurbs_saddle.fbx` (Maya cm → ~cm after ModifyGeometry).
//! Look for: smooth saddle surface spinning (tessellated triangles, not raw NURBS).

use std::f32::consts::PI;

use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "nurbs_saddle.fbx";
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
                title: "bevy_ufbx — nurbs_fbx".into(),
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
    commands.spawn((
        WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(FBX_PATH))),
        Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
        Spinning,
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(2.5, 2.2, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.45, 0.35))),
        Transform::from_xyz(0.0, -0.6, 0.0),
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
            "nurbs_fbx — nurbs_saddle.fbx\n\
             Look for: tessellated saddle surface (triangle mesh), spinning.\n\
             Native NURBS is not rendered; loader tessellates via ufbx.",
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
        t.rotate_y(time.delta_secs() * 0.45);
    }
}
