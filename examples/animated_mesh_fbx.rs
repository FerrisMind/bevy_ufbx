//! Plays a baked FBX animation via [`AnimationPlayer`] + [`AnimationGraph`].
//!
//! ```sh
//! cargo run --example animated_mesh_fbx
//! ```
//!
//! Asset: `assets/cube_anim.fbx` (ufbx test fixture with a transform take).
//! Every TRS channel is authored `Cubic|TangeantAuto`, so the clip carries the
//! loader's cubic conversions — `CubicKeyframeCurve` (translation/scale) and
//! `CubicRotationCurve` (rotation) — not the baked-linear fallback. The HUD
//! prints a live curve census (`tests/animation_cubic.rs` pins the same fact).
//! The loader exposes clips; this example builds the graph and starts playback
//! (no loader auto-play).
//!
//! Look for: the cube translating + rotating on a loop, plus the HUD line
//! (`AnimationPlayers` / `seek` / node `T=`) changing over frames.
//!
//! Framing: the camera fits the measured union `Aabb` of the spawned meshes,
//! sampled across ~1.5 s of playback and printed to stdout — this example used
//! to hard-code a camera against a cube the take moves around.
//!
//! Note: `FbxLoaderSettings::generate_rest_animation` can emit an `AnimationRest`
//! / `"Rest"` bind-pose clip for character workflows; this demo plays take 0 only.

use std::f32::consts::PI;

use bevy::animation::AnimationClip;
use bevy::asset::LoadState;
use bevy::camera::primitives::Aabb;
use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, window::PrimaryWindow,
    world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "cube_anim.fbx";
/// Maya cm fixture → ~1 cm after ModifyGeometry; enlarge for demo framing only.
const DEMO_VISUAL_SCALE: f32 = 100.0;

/// Frames a few frames after the instance spawns: the loader inserts `Aabb`
/// with the spawn, but `GlobalTransform` propagation needs a frame or two.
const FRAMING_SETTLE_FRAMES: u32 = 3;
/// Union window in frames (~1.5 s at 60 fps — at least one full 0.833 s take
/// loop) so the fitted bounds cover the cube wherever the take moves it.
const FRAMING_SAMPLE_FRAMES: u32 = 90;
/// Original hard-coded viewpoint direction — kept, only the distance is fitted.
const VIEW_DIR: Vec3 = Vec3::new(2.5, 2.0, 4.0);
/// Bounding-sphere margin on the fitted distance (× slack for the moving cube).
const FRAMING_MARGIN: f32 = 1.3;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(FbxPlugin)
        .init_resource::<CameraFraming>()
        .add_systems(Startup, setup_mesh_and_animation)
        .add_systems(Startup, setup_camera_and_environment)
        .add_systems(Update, (frame_camera_when_ready, update_status))
        .run();
}

#[derive(Component)]
struct AnimationToPlay {
    graph_handle: Handle<AnimationGraph>,
    index: AnimationNodeIndex,
}

/// Label-less `Fbx` load (take names via `Fbx::named_animations`) + the clip
/// the HUD reports on.
#[derive(Resource)]
struct DemoHandles {
    fbx: Handle<Fbx>,
    clip: Handle<AnimationClip>,
}

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct ViewerCamera;

#[derive(Component)]
struct Floor;

/// Fit-to-content state machine: armed by the `WorldInstanceReady` observer,
/// unions mesh `Aabb`s across a post-spawn window, then places the camera once.
/// Only meshes UNDER the scene instance count — the floor and Bevy's UI glyph
/// meshes live in the same world and would inflate the bounds otherwise.
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
    root: Option<Entity>,
    union_min: Vec3,
    union_max: Vec3,
    meshes: u32,
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
    let clip_handle = asset_server.load(FbxAssetLabel::Animation(0).from_asset(FBX_PATH));

    commands.insert_resource(DemoHandles {
        fbx: asset_server.load(FBX_PATH),
        clip: clip_handle,
    });

    commands
        .spawn((
            AnimationToPlay {
                graph_handle,
                index,
            },
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(FBX_PATH))),
            Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
        ))
        .observe(play_animation_when_ready);

    spawn_hud(
        &mut commands,
        format!("animated_mesh_fbx — {FBX_PATH}\nLoading clip…"),
    );
}

fn play_animation_when_ready(
    scene_ready: On<WorldInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    animations_to_play: Query<&AnimationToPlay>,
    mut players: Query<&mut AnimationPlayer>,
    mut framing: ResMut<CameraFraming>,
) {
    let Ok(animation_to_play) = animations_to_play.get(scene_ready.entity) else {
        return;
    };
    let mut started = 0usize;
    for child in children.iter_descendants(scene_ready.entity) {
        if let Ok(mut player) = players.get_mut(child) {
            player.play(animation_to_play.index).repeat();
            commands
                .entity(child)
                .insert(AnimationGraphHandle(animation_to_play.graph_handle.clone()));
            started += 1;
        }
    }
    // The loader must have spawned an `AnimationPlayer` on the animation root —
    // without it the graph handle has nowhere to go and nothing would play.
    assert!(
        started > 0,
        "no AnimationPlayer under the {FBX_PATH} scene root — loader must insert one"
    );
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
        framing.root = Some(scene_ready.entity);
        framing.union_min = Vec3::splat(f32::INFINITY);
        framing.union_max = Vec3::splat(f32::NEG_INFINITY);
        framing.meshes = 0;
    }
}

fn setup_camera_and_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // near=0.01 (Bevy default 0.1) so the fit-to-content camera can approach
    // cm-scaled fixtures; Bevy's reverse-infinite-Z keeps depth fine.
    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_xyz(VIEW_DIR.x, VIEW_DIR.y, VIEW_DIR.z).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
        Floor,
        Transform::from_xyz(0.0, -0.55, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.)),
        DirectionalLight {
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

/// Fit the camera to the union world-space `Aabb` of all spawned meshes,
/// accumulated across [`FRAMING_SAMPLE_FRAMES`] frames so the moving cube stays
/// framed for the whole take. Falls back to the default framing on timeout.
fn frame_camera_when_ready(
    mut framing: ResMut<CameraFraming>,
    mut camera: Query<(&mut Transform, &Projection), With<ViewerCamera>>,
    meshes: Query<(&Aabb, &GlobalTransform), With<Mesh3d>>,
    children: Query<&Children>,
    // `Without` keeps the two `&mut Transform` queries provably disjoint (B0001).
    mut floors: Query<&mut Transform, (With<Floor>, Without<ViewerCamera>)>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !framing.armed || framing.done {
        return;
    }
    framing.frames += 1;
    if framing.frames < FRAMING_SETTLE_FRAMES {
        return;
    }
    let Some(root) = framing.root else {
        return;
    };

    // Union of world-space mesh bounds, restricted to the scene instance —
    // the floor plane and UI glyph meshes are not part of the content.
    // `Aabb` is inserted by the loader (and by Bevy's calculate_bounds one
    // frame after spawn) — extend every frame of the sampling window so
    // animated motion is covered too.
    for entity in children.iter_descendants(root) {
        let Ok((aabb, global)) = meshes.get(entity) else {
            continue;
        };
        let lo = aabb.min();
        let hi = aabb.max();
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for z in [lo.z, hi.z] {
                    let world = global.transform_point(Vec3::new(x, y, z));
                    framing.union_min = framing.union_min.min(world);
                    framing.union_max = framing.union_max.max(world);
                }
            }
        }
        // Count mesh entities once (on the first sampling frame), not per frame.
        if framing.frames == FRAMING_SETTLE_FRAMES {
            framing.meshes += 1;
        }
    }
    if framing.frames < FRAMING_SETTLE_FRAMES + FRAMING_SAMPLE_FRAMES {
        return;
    }
    framing.armed = false;
    framing.done = true;
    if framing.meshes == 0 {
        println!(
            "[animated_mesh_fbx] no mesh bounds in the {}-frame sampling window — \
             keeping default camera framing",
            FRAMING_SETTLE_FRAMES + FRAMING_SAMPLE_FRAMES
        );
        return;
    }

    let center = (framing.union_min + framing.union_max) * 0.5;
    let radius = ((framing.union_max - framing.union_min) * 0.5)
        .length()
        .max(1e-5);

    let Ok((mut cam_tf, projection)) = camera.single_mut() else {
        return;
    };
    let Projection::Perspective(p) = projection else {
        return;
    };
    let (v_fov, near) = (p.fov, p.near);
    let aspect = windows
        .single()
        .map(|w| {
            let h = w.resolution.height();
            if h > 0.0 {
                w.resolution.width() / h
            } else {
                1.0
            }
        })
        .unwrap_or(1.0)
        .max(0.1);
    let h_fov = 2.0 * ((0.5 * v_fov).tan() * aspect).atan();
    let half_angle = 0.5 * v_fov.min(h_fov);
    let mut distance = radius / half_angle.sin() * FRAMING_MARGIN;
    distance = distance.max(near * 1.5 + radius);

    let pos = center + VIEW_DIR.normalize() * distance;
    *cam_tf = Transform::from_translation(pos).looking_at(center, Vec3::Y);

    // Keep the floor under the (possibly moving) content.
    if let Ok(mut floor) = floors.single_mut() {
        floor.translation = Vec3::new(center.x, framing.union_min.y - 0.02, center.z);
    }

    println!(
        "[animated_mesh_fbx] content bounds: {} mesh AABB(s), union sampled over {} frames",
        framing.meshes, framing.frames
    );
    println!(
        "[animated_mesh_fbx]   world min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4}) \
         center=({:.4}, {:.4}, {:.4}) radius={:.4}",
        framing.union_min.x,
        framing.union_min.y,
        framing.union_min.z,
        framing.union_max.x,
        framing.union_max.y,
        framing.union_max.z,
        center.x,
        center.y,
        center.z,
        radius
    );
    println!(
        "[animated_mesh_fbx] camera framing: distance={distance:.4} v_fov={v_fov:.4}rad \
         aspect={aspect:.3} near={near:.4} translation=({:.4}, {:.4}, {:.4})",
        pos.x, pos.y, pos.z
    );
}

/// Live HUD: static clip facts once `Fbx` + `AnimationClip` finish loading,
/// then the player's elapsed time and the animated node's translation so the
/// take's motion is verifiable frame to frame.
fn update_status(
    asset_server: Res<AssetServer>,
    handles: Res<DemoHandles>,
    fbxs: Res<Assets<Fbx>>,
    clips: Res<Assets<AnimationClip>>,
    players: Query<(&AnimationPlayer, &Transform)>,
    mut texts: Query<&mut Text, With<StatusText>>,
    mut facts: Local<Option<String>>,
    mut printed: Local<bool>,
) {
    let mut lines = vec![format!("animated_mesh_fbx — {FBX_PATH}")];
    lines.push("Look for: cube TRANSLATING + ROTATING (cubic TRS take, looped).".into());

    if facts.is_none() {
        let fbx_state = asset_server.load_state(&handles.fbx);
        let clip_state = asset_server.load_state(&handles.clip);
        if matches!(clip_state, LoadState::Failed(_)) {
            lines.push("Animation0 clip FAILED to load — see stderr".into());
        } else if matches!(fbx_state, LoadState::Loaded)
            && matches!(clip_state, LoadState::Loaded)
            && let (Some(fbx), Some(clip)) = (fbxs.get(&handles.fbx), clips.get(&handles.clip))
        {
            let index = fbx.animations.iter().position(|h| *h == handles.clip);
            let take = fbx
                .named_animations
                .iter()
                .find(|(_, h)| **h == handles.clip)
                .map(|(name, _)| name.to_string());
            let label = index.map_or_else(|| "Animation?".into(), |i| format!("Animation{i}"));
            let take = take.unwrap_or_else(|| "unnamed take".into());
            // Curve census via Debug strings — same trick as tests/animation_cubic.rs.
            let (mut total, mut cubic_t, mut cubic_r) = (0usize, 0usize, 0usize);
            for curves in clip.curves().values() {
                for curve in curves {
                    total += 1;
                    let debug = format!("{curve:?}");
                    if debug.contains("CubicKeyframeCurve") {
                        cubic_t += 1;
                    } else if debug.contains("CubicRotationCurve") {
                        cubic_r += 1;
                    }
                }
            }
            let line = format!(
                "take \"{take}\" → {label}: {:.3}s, {total} curves \
                 ({cubic_t}× CubicKeyframeCurve + {cubic_r}× CubicRotationCurve = cubic)",
                clip.duration()
            );
            if !*printed {
                *printed = true;
                println!("[animated_mesh_fbx] {line}");
            }
            *facts = Some(line);
        } else {
            lines.push("loading clip…".into());
        }
    }
    if let Some(line) = facts.as_ref() {
        lines.push(line.clone());
    }

    let (mut count, mut seek, mut node_t) = (0usize, 0.0, Vec3::ZERO);
    for (player, transform) in &players {
        count += 1;
        for (_, active) in player.playing_animations() {
            seek = active.seek_time();
        }
        node_t = transform.translation;
    }
    lines.push(format!(
        "AnimationPlayers: {count} | seek {seek:.2}s | node T=({:.3}, {:.3}, {:.3})",
        node_t.x, node_t.y, node_t.z
    ));

    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(lines.join("\n"));
    }
}

fn spawn_hud(commands: &mut Commands, text: String) {
    commands.spawn((
        StatusText,
        Text::new(text),
        TextFont::from_font_size(17.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        // Translucent plate: the HUD stays legible over bright meshes.
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
    ));
}
