//! Negative world scale → cull-inverted `Material{N} (inverted)`.
//!
//! ```sh
//! cargo run --example neg_scale_fbx
//! ```
//!
//! Asset: `assets/blender_340_mirrored_normals_7400_binary.fbx`
//! Contains `Suzanne_Flipped` with local scale `(-1,-1,-1)` (odd negative axes
//! on world scale). The loader selects the Front-cull twin so the mirrored mesh
//! is not inside-out.
//!
//! Look for: two Suzannes (normal + mirrored); mirrored uses inverted cull.

use std::f32::consts::PI;
use std::path::Path;

use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, render::render_resource::Face,
    world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "blender_340_mirrored_normals_7400_binary.fbx";
/// Blender Auto → AdjustTransforms; metre-ish extents.
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
                title: "bevy_ufbx — neg_scale_fbx".into(),
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
        .observe(report_inverted_cull);

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(4.5, 2.8, 5.5).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.28, 0.4, 0.32))),
        Transform::from_xyz(0.0, -1.2, 0.0),
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
            "neg_scale_fbx — {FBX_PATH}\n\
             Look for: mirrored Suzanne with Front-cull (Material inverted twin).\n\
             Counting Mesh3d / Front cull after spawn…"
        )),
        TextFont::from_font_size(17.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

fn report_inverted_cull(
    ready: On<WorldInstanceReady>,
    children: Query<&Children>,
    mesh3d: Query<(), With<Mesh3d>>,
    mesh_mats: Query<&MeshMaterial3d<StandardMaterial>>,
    materials: Res<Assets<StandardMaterial>>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    let mut mesh_count = 0usize;
    let mut front_cull = 0usize;
    for child in children.iter_descendants(ready.entity) {
        if mesh3d.get(child).is_ok() {
            mesh_count += 1;
        }
        if let Ok(mm) = mesh_mats.get(child)
            && let Some(mat) = materials.get(&mm.0)
            && mat.cull_mode == Some(Face::Front)
        {
            front_cull += 1;
        }
    }
    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "neg_scale_fbx — {FBX_PATH}\n\
             Look for: normal + mirrored Suzanne; mirrored uses inverted cull.\n\
             Scene0 Mesh3d={mesh_count}; Mesh3d with Front cull (inverted)={front_cull}.\n\
             Loader picks Material{{N}} (inverted) when world scale has odd neg axes."
        ));
    }
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.35);
    }
}
