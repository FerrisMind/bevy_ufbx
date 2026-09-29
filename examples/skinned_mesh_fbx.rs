//! Skinned mesh deformation from FBX (LBS + AnimationGraph).
//!
//! ```sh
//! cargo run --example skinned_mesh_fbx
//! ```
//!
//! Prefer `assets/blender_279_sausage_7400_binary.fbx` (skinned + takes).
//! Falls back to `assets/rigged_triangle.fbx` if sausage is missing.
//!
//! Look for: mesh deforming with joint animation (Wiggle/Spin), not a rigid spin.

use std::f32::consts::PI;
use std::path::Path;

use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const SAUSAGE: &str = "blender_279_sausage_7400_binary.fbx";
const RIGGED_TRI: &str = "rigged_triangle.fbx";
/// Maya/cm-style fallback is centimetre-sized after ModifyGeometry.
const TRI_DEMO_SCALE: f32 = 100.0;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — skinned_mesh_fbx".into(),
                ..default()
            }),
            ..default()
        }))
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

fn pick_fbx() -> (&'static str, f32, usize) {
    if Path::new("assets").join(SAUSAGE).is_file() {
        // Blender AdjustTransforms → metre-ish; anim[2] = Skeleton|Wiggle.
        (SAUSAGE, 1.0, 2)
    } else {
        (RIGGED_TRI, TRI_DEMO_SCALE, 0)
    }
}

fn setup_mesh_and_animation(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    let (fbx_path, scale, anim_index) = pick_fbx();
    let (graph, index) = AnimationGraph::from_clip(
        asset_server.load(FbxAssetLabel::Animation(anim_index).from_asset(fbx_path)),
    );
    let graph_handle = graphs.add(graph);

    commands
        .spawn((
            AnimationToPlay {
                graph_handle,
                index,
            },
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(fbx_path))),
            Transform::from_scale(Vec3::splat(scale)),
        ))
        .observe(play_when_ready);

    spawn_hud(
        &mut commands,
        format!(
            "skinned_mesh_fbx — {fbx_path}\n\
             Look for: skinned mesh DEFORMING (joints / LBS), not a rigid body spin.\n\
             AnimationGraph plays take #{anim_index} (no loader auto-play)."
        ),
    );
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
    let (_, scale, _) = pick_fbx();
    let cam_dist = if scale > 10.0 { 4.0 } else { 3.5 };
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(cam_dist, cam_dist * 0.8, cam_dist * 1.4).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.32, 0.42, 0.35))),
        Transform::from_xyz(0.0, -0.05, 0.0),
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
}

fn spawn_hud(commands: &mut Commands, text: String) {
    commands.spawn((
        Text::new(text),
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
