//! Plays morph / blend-shape weights from an FBX file.
//!
//! ```sh
//! cargo run --example morph_fbx
//! ```
//!
//! Asset: `assets/blend_shape_cube.fbx` (ufbx Maya fixture).
//!
//! Measured facts (do not “guess” framing):
//! - File `unit_meters = 0.01` (centimetres).
//! - With `SpaceConversion::AdjustTransforms`, mesh verts stay ±0.5 and the
//!   **node** local scale becomes `0.01` → world size ≈ **1 cm**.
//! - Root scale `100` cancels that → ≈ 1 m cube on screen.

use std::f32::consts::PI;

use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "blend_shape_cube.fbx";
/// Inverse of the 0.01 node scale left by cm→m `AdjustTransforms`.
const CM_TO_METRE_ROOT_SCALE: f32 = 100.0;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(FbxPlugin)
        .add_systems(Startup, setup_mesh_and_animation)
        .add_systems(Startup, setup_camera_and_environment)
        .run();
}

#[derive(Component)]
struct AnimationToPlay {
    graph_handle: Handle<AnimationGraph>,
    index: AnimationNodeIndex,
}

fn setup_mesh_and_animation(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    let (graph, index) = AnimationGraph::from_clip(
        asset_server.load(FbxAssetLabel::Animation(0).from_asset(FBX_PATH)),
    );
    let graph_handle = graphs.add(graph);

    commands
        .spawn((
            AnimationToPlay {
                graph_handle,
                index,
            },
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(FBX_PATH))),
            Transform::from_scale(Vec3::splat(CM_TO_METRE_ROOT_SCALE)),
        ))
        .observe(play_when_ready);
}

fn play_when_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    to_play: Query<&AnimationToPlay>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let Ok(anim) = to_play.get(ready.entity) else {
        return;
    };
    for child in children.iter_descendants(ready.entity) {
        if let Ok(mut player) = players.get_mut(child) {
            player.play(anim.index).repeat();
            commands
                .entity(child)
                .insert(AnimationGraphHandle(anim.graph_handle.clone()));
        }
    }
}

fn setup_camera_and_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Framed for a ~1 m cube (after CM_TO_METRE_ROOT_SCALE).
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(2.5, 2.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.35, 0.55, 0.35))),
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
}
