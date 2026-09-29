//! Side-by-side visual coverage checklist for native FBX features.
//!
//! ```sh
//! cargo run --example showcase_fbx
//! ```
//!
//! Spawns multiple `WorldAssetRoot`s: morph, TRS anim, NURBS, skin, multimaterial.
//! Look for: five labeled demos in one window (coverage checklist).

use std::f32::consts::PI;

use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const MORPH: &str = "blend_shape_cube.fbx";
const ANIM: &str = "cube_anim.fbx";
const NURBS: &str = "nurbs_saddle.fbx";
const SKIN: &str = "blender_279_sausage_7400_binary.fbx";
const SKIN_FALLBACK: &str = "rigged_triangle.fbx";
const MULTI: &str = "blender_suzanne_multimaterial_7400_binary.fbx";

const CM_SCALE: f32 = 80.0;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 1800.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — showcase_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .add_systems(Startup, setup)
        .run();
}

#[derive(Component)]
struct AnimationToPlay {
    graph_handle: Handle<AnimationGraph>,
    index: AnimationNodeIndex,
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: 0,
            ..default()
        },
        Transform::from_xyz(0.0, 3.8, 11.0).looking_at(Vec3::new(0.0, 0.4, 0.0), Vec3::Y),
    ));

    // Overlay camera for Text2d name plates (does not clear the 3D color buffer).
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(24.0, 10.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.26, 0.34, 0.28))),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 0.8, -PI / 4.)),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 30.0,
            maximum_distance: 60.0,
            ..default()
        }
        .build(),
    ));

    // Screen-space name plates (Text2d under Camera2d). Five slots L→R.
    let plates = [
        (-420.0, "morph"),
        (-210.0, "anim TRS"),
        (0.0, "nurbs"),
        (210.0, "skin"),
        (420.0, "multi-mat"),
    ];
    for (x, label) in plates {
        commands.spawn((
            Text2d::new(label),
            TextFont::from_font_size(26.0),
            TextColor(Color::srgb(1.0, 0.95, 0.7)),
            TextLayout::justify(Justify::Center),
            Transform::from_xyz(x, 280.0, 0.0),
        ));
    }

    spawn_animated(
        &mut commands,
        &asset_server,
        &mut graphs,
        MORPH,
        0,
        Vec3::new(-5.5, 0.0, 0.0),
        CM_SCALE,
    );
    spawn_animated(
        &mut commands,
        &asset_server,
        &mut graphs,
        ANIM,
        0,
        Vec3::new(-2.75, 0.0, 0.0),
        CM_SCALE,
    );
    commands.spawn((
        WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(NURBS))),
        Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)).with_scale(Vec3::splat(CM_SCALE)),
    ));

    let (skin_path, skin_scale, skin_anim) =
        if std::path::Path::new("assets").join(SKIN).is_file() {
            (SKIN, 0.85, 2usize)
        } else {
            (SKIN_FALLBACK, CM_SCALE, 0usize)
        };
    spawn_animated(
        &mut commands,
        &asset_server,
        &mut graphs,
        skin_path,
        skin_anim,
        Vec3::new(2.75, 0.0, 0.0),
        skin_scale,
    );

    // Blender multimaterial Suzanne (AdjustTransforms → metre-ish).
    let multi_scale = if std::path::Path::new("assets").join(MULTI).is_file() {
        0.9
    } else {
        0.0
    };
    if multi_scale > 0.0 {
        commands.spawn((
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(MULTI))),
            Transform::from_translation(Vec3::new(5.5, 0.0, 0.0))
                .with_scale(Vec3::splat(multi_scale)),
        ));
    }

    commands.spawn((
        Text::new(
            "showcase_fbx — visual coverage checklist\n\
             LEFT→RIGHT: morph | anim TRS | nurbs | skin | multi-mat\n\
             Each slot is its own WorldAssetRoot (Scene0). AnimationGraph only (no auto-play).",
        ),
        TextFont::from_font_size(18.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

fn spawn_animated(
    commands: &mut Commands,
    asset_server: &AssetServer,
    graphs: &mut Assets<AnimationGraph>,
    path: &'static str,
    anim_index: usize,
    translation: Vec3,
    scale: f32,
) {
    let (graph, index) = AnimationGraph::from_clip(
        asset_server.load(FbxAssetLabel::Animation(anim_index).from_asset(path)),
    );
    let graph_handle = graphs.add(graph);

    commands
        .spawn((
            AnimationToPlay {
                graph_handle,
                index,
            },
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(path))),
            Transform::from_translation(translation).with_scale(Vec3::splat(scale)),
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
