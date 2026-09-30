//! Plays morph / blend-shape weights from an FBX file.
//!
//! ```sh
//! cargo run --example morph_fbx
//! ```
//!
//! Asset: `assets/blend_shape_cube.fbx` (Maya cm fixture from ufbx testdata).
//! `DeformPercent` is authored with cubic keys, so the clip's weight track is
//! the loader's private `WideHermiteCurve` — the workaround for the
//! `WideCubicKeyframeCurve` defects in bevy_animation 0.19.1
//! (`tests/animation_cubic.rs` pins both). The HUD prints the curve type it
//! actually found plus the live `MorphWeights` values, so the morph motion is
//! verifiable frame to frame.
//!
//! Look for: the cube's shape morphing (taper → cube → …) while the HUD
//! `morph weights` line changes.
//!
//! With the default [`bevy_ufbx::FbxSpaceConversion::Auto`] (this Maya fixture
//! resolves to `ModifyGeometry`), the mesh is correctly ~**1 cm** in metres and
//! node scale is identity. This example applies a visual root scale so the
//! centimetre fixture is readable in a metre-ish scene; the camera fits the
//! measured union `Aabb` of the spawned meshes (printed to stdout).

use std::f32::consts::PI;

use bevy::animation::AnimationClip;
use bevy::asset::LoadState;
use bevy::camera::primitives::Aabb;
use bevy::mesh::morph::MorphWeights;
use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, window::PrimaryWindow,
    world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "blend_shape_cube.fbx";
/// Maya fixture is a 1 cm cube after metre conversion — enlarge for the demo only.
const DEMO_VISUAL_SCALE: f32 = 100.0;

/// Frames a few frames after the instance spawns: the loader inserts `Aabb`
/// with the spawn, but `GlobalTransform` propagation needs a frame or two.
const FRAMING_SETTLE_FRAMES: u32 = 3;
/// Union window in frames — the morph take (4.96 s) does not move the node,
/// a short window is enough to cover spawn-time motion.
const FRAMING_SAMPLE_FRAMES: u32 = 30;
/// Original hard-coded viewpoint direction — kept, only the distance is fitted.
const VIEW_DIR: Vec3 = Vec3::new(2.5, 2.0, 4.0);
/// Bounding-sphere margin on the fitted distance (× slack for morph growth —
/// `Aabb` tracks the base mesh, not the morphed shape).
const FRAMING_MARGIN: f32 = 1.35;

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

    commands.insert_resource(DemoHandles {
        fbx: asset_server.load(FBX_PATH),
        clip: asset_server.load(FbxAssetLabel::Animation(0).from_asset(FBX_PATH)),
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
        .observe(play_when_ready);

    spawn_hud(
        &mut commands,
        format!("morph_fbx — {FBX_PATH}\nLoading clip…"),
    );
}

fn play_when_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    to_play: Query<&AnimationToPlay>,
    mut players: Query<&mut AnimationPlayer>,
    morphs: Query<&MorphWeights>,
    mesh3d: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
    mut framing: ResMut<CameraFraming>,
) {
    let Ok(anim) = to_play.get(ready.entity) else {
        return;
    };
    let mut started = 0usize;
    let mut morph_nodes = 0usize;
    let mut morph_meshes = 0usize;
    for child in children.iter_descendants(ready.entity) {
        if let Ok(mut player) = players.get_mut(child) {
            player.play(anim.index).repeat();
            commands
                .entity(child)
                .insert(AnimationGraphHandle(anim.graph_handle.clone()));
            started += 1;
        }
        if morphs.contains(child) {
            morph_nodes += 1;
        }
        if let Ok(m3d) = mesh3d.get(child)
            && let Some(mesh) = meshes.get(&m3d.0)
            && mesh.morph_targets().is_some()
        {
            morph_meshes += 1;
        }
    }
    // The demo claims morph animation — require every piece of the chain:
    // a player to drive the clip, `MorphWeights` on the node, and a mesh
    // carrying the morph target buffer.
    assert!(
        started > 0,
        "no AnimationPlayer under the {FBX_PATH} scene root — loader must insert one"
    );
    assert!(
        morph_nodes > 0,
        "no MorphWeights node under the {FBX_PATH} scene root — loader must insert one"
    );
    assert!(
        morph_meshes > 0,
        "no mesh with morph targets under the {FBX_PATH} scene root"
    );
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
        framing.root = Some(ready.entity);
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
        MeshMaterial3d(materials.add(Color::srgb(0.35, 0.55, 0.35))),
        Floor,
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

/// Fit the camera to the union world-space `Aabb` of all spawned meshes,
/// accumulated across [`FRAMING_SAMPLE_FRAMES`] frames. Falls back to the
/// default framing if no bounds appear.
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
    // frame after spawn).
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
            "[morph_fbx] no mesh bounds in the {}-frame sampling window — \
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

    // Keep the floor under the content.
    if let Ok(mut floor) = floors.single_mut() {
        floor.translation = Vec3::new(center.x, framing.union_min.y - 0.02, center.z);
    }

    println!(
        "[morph_fbx] content bounds: {} mesh AABB(s), union sampled over {} frames",
        framing.meshes,
        framing.frames
    );
    println!(
        "[morph_fbx]   world min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4}) \
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
        "[morph_fbx] camera framing: distance={distance:.4} v_fov={v_fov:.4}rad \
         aspect={aspect:.3} near={near:.4} translation=({:.4}, {:.4}, {:.4})",
        pos.x, pos.y, pos.z
    );
}

/// Live HUD: static clip facts once `Fbx` + `AnimationClip` finish loading,
/// then the live `MorphWeights` values so the morph motion is verifiable.
fn update_status(
    asset_server: Res<AssetServer>,
    handles: Res<DemoHandles>,
    fbxs: Res<Assets<Fbx>>,
    clips: Res<Assets<AnimationClip>>,
    mesh3d: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
    morphs: Query<&MorphWeights>,
    players: Query<&AnimationPlayer>,
    mut texts: Query<&mut Text, With<StatusText>>,
    mut facts: Local<Option<String>>,
    mut printed: Local<bool>,
) {
    let mut lines = vec![format!("morph_fbx — {FBX_PATH}")];
    lines.push("Look for: cube shape MORPHING (blend-shape weights, cubic keys).".into());

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
            let mut total = 0usize;
            let mut wide_hermite = 0usize;
            let mut wide_linear = 0usize;
            let mut other = 0usize;
            for curves in clip.curves().values() {
                for curve in curves {
                    total += 1;
                    let debug = format!("{curve:?}");
                    if debug.contains("WideHermiteCurve") {
                        wide_hermite += 1;
                    } else if debug.contains("WideLinearKeyframeCurve") {
                        wide_linear += 1;
                    } else {
                        other += 1;
                    }
                }
            }
            let line = format!(
                "take \"{take}\" → {label}: {:.3}s, {total} curve(s): \
                 {wide_hermite}× WideHermiteCurve (cubic morph) + {wide_linear}× WideLinear + {other}× other",
                clip.duration()
            );
            if !*printed {
                *printed = true;
                println!("[morph_fbx] {line}");
            }
            *facts = Some(line);
        } else {
            lines.push("loading clip…".into());
        }
    }
    if let Some(line) = facts.as_ref() {
        lines.push(line.clone());
    }

    // Morph target names from the first morphing mesh asset.
    let mut names_line = "morph targets: (loading…)".to_string();
    for m3d in &mesh3d {
        if let Some(mesh) = meshes.get(&m3d.0)
            && let Some(names) = mesh.morph_target_names()
        {
            names_line = format!("morph targets: {} ({})", names.len(), names.join(", "));
            break;
        }
    }
    lines.push(names_line);

    // Live weights + player count.
    let mut weights_line = "morph weights: (none yet)".to_string();
    for mw in &morphs {
        let weights: Vec<String> = mw
            .weights()
            .iter()
            .map(|w| format!("{w:.3}"))
            .collect();
        weights_line = format!("morph weights: [{}]", weights.join(", "));
        break;
    }
    let players_count = players.iter().count();
    lines.push(format!("{weights_line} | players: {players_count}"));

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
