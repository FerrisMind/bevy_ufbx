//! Material-split primitives on one mesh (multi-material Suzanne).
//!
//! ```sh
//! cargo run --example multimaterial_fbx
//! ```
//!
//! Asset: `assets/blender_suzanne_multimaterial_7400_binary.fbx`
//! Look for: one Suzanne with several color regions (7 Mesh3d / materials).
//! Blender Auto → AdjustTransforms; demo scale stays 1.0 (metre-ish extents).

use std::f32::consts::PI;
use std::path::Path;

use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "blender_suzanne_multimaterial_7400_binary.fbx";
/// Blender AdjustTransforms → metre-ish; keep 1.0 and frame with a close camera.
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
                title: "bevy_ufbx — multimaterial_fbx".into(),
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
        Transform::from_xyz(3.2, 2.4, 4.2).looking_at(Vec3::new(0.0, 0.2, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.28, 0.4, 0.32))),
        Transform::from_xyz(0.0, -1.0, 0.0),
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
        StatusText,
        Text::new(format!(
            "multimaterial_fbx — {FBX_PATH}\n\
             Look for: COLOR REGIONS on Suzanne (material-split primitives).\n\
             Expect ≥2 Mesh3d entities under Scene0 (dump shows 7).\n\
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
            "multimaterial_fbx — {FBX_PATH}\n\
             Look for: COLOR REGIONS on Suzanne (material-split primitives).\n\
             Scene0 Mesh3d count: {count} (material-split = multiple Mesh3d).\n\
             Blender Auto → AdjustTransforms; DEMO_VISUAL_SCALE={DEMO_VISUAL_SCALE}."
        ));
    }
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.35);
    }
}
