//! Nested mesh hierarchy (parent/child mesh nodes under one Scene0).
//!
//! ```sh
//! cargo run --example nested_meshes_fbx
//! ```
//!
//! Asset: `assets/blender_279_nested_meshes_7400_binary.fbx`
//! Look for: several distinct shapes (cube / cone / icosphere / plane) spinning
//! together as one hierarchy — not a single fused mesh.
//! Blender Auto → AdjustTransforms; demo scale 1.0.

use std::f32::consts::PI;
use std::path::Path;

use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "blender_279_nested_meshes_7400_binary.fbx";
const DEMO_VISUAL_SCALE: f32 = 1.0;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — nested_meshes_fbx".into(),
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

#[derive(Component)]
struct StatusText;

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    assert!(
        Path::new("assets").join(FBX_PATH).is_file(),
        "missing assets/{FBX_PATH} — copy from libs/ufbx/data"
    );

    commands
        .spawn((
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(FBX_PATH))),
            Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
            Spinning,
        ))
        .observe(report_mesh3d_count);

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(4.0, 3.0, 5.5).looking_at(Vec3::new(0.0, 0.3, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.26, 0.32, 0.36))),
        Transform::from_xyz(0.0, -1.2, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 0.9, -PI / 4.)),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 25.0,
            maximum_distance: 50.0,
            ..default()
        }
        .build(),
    ));

    commands.spawn((
        StatusText,
        Text::new(format!(
            "nested_meshes_fbx — {FBX_PATH}\n\
             Look for: MULTIPLE shapes in one hierarchy (cube, cone, icosphere, plane).\n\
             Proves nested mesh nodes → separate Mesh3d under Scene0.\n\
             Counting Mesh3d after spawn…"
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

fn report_mesh3d_count(
    ready: On<WorldInstanceReady>,
    children: Query<&Children>,
    mesh3d: Query<(), With<Mesh3d>>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    let mut count = 0usize;
    for child in children.iter_descendants(ready.entity) {
        if mesh3d.get(child).is_ok() {
            count += 1;
        }
    }
    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "nested_meshes_fbx — {FBX_PATH}\n\
             Look for: MULTIPLE shapes in one hierarchy (cube, cone, icosphere, plane).\n\
             Scene0 Mesh3d count: {count} (dump expects 4 nested meshes).\n\
             Blender Auto → AdjustTransforms; DEMO_VISUAL_SCALE={DEMO_VISUAL_SCALE}."
        ));
    }
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.35);
    }
}
