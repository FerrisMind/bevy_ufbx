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
//! The observer asserts the spawned scene really carries a `SkinnedMesh` with
//! joints, and the HUD prints the joint count + live animation seek time so the
//! claim is checkable on screen. Framing: the camera fits the measured union
//! `Aabb` of the spawned meshes (printed to stdout) — the old hard-coded camera
//! clipped the 6-unit sausage mesh.

use std::f32::consts::PI;
use std::path::Path;

use bevy::animation::AnimationClip;
use bevy::asset::LoadState;
use bevy::camera::primitives::Aabb;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, window::PrimaryWindow,
    world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxPlugin};

const SAUSAGE: &str = "blender_279_sausage_7400_binary.fbx";
const RIGGED_TRI: &str = "rigged_triangle.fbx";
/// Maya/cm-style fallback is centimetre-sized after ModifyGeometry.
const TRI_DEMO_SCALE: f32 = 100.0;

/// Frames a few frames after the instance spawns: the loader inserts `Aabb`
/// with the spawn, but `GlobalTransform` propagation needs a frame or two.
const FRAMING_SETTLE_FRAMES: u32 = 3;
/// Union window in frames (~1.5 s at 60 fps — at least one full 0.79 s Wiggle
/// loop) so the fitted bounds cover the mesh wherever the take deforms it.
const FRAMING_SAMPLE_FRAMES: u32 = 90;
/// Original hard-coded viewpoint direction — kept, only the distance is fitted.
const VIEW_DIR: Vec3 = Vec3::new(3.5, 2.8, 4.9);
/// Bounding-sphere margin on the fitted distance (× slack for the deforming skin).
const FRAMING_MARGIN: f32 = 1.3;

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
    path: &'static str,
    anim_index: usize,
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
    let (graph, index) = AnimationGraph::from_clip(asset_server.load(
        FbxAssetLabel::Animation(anim_index).from_asset(fbx_path),
    ));
    let graph_handle = graphs.add(graph);

    commands.insert_resource(DemoHandles {
        fbx: asset_server.load(fbx_path),
        clip: asset_server.load(FbxAssetLabel::Animation(anim_index).from_asset(fbx_path)),
        path: fbx_path,
        anim_index,
    });

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
    skinned: Query<&SkinnedMesh>,
    handles: Res<DemoHandles>,
    mut framing: ResMut<CameraFraming>,
) {
    let Ok(anim) = to_play.get(ready.entity) else {
        return;
    };
    let mut started = 0usize;
    let mut skinned_meshes = 0usize;
    let mut joints = 0usize;
    for child in children.iter_descendants(ready.entity) {
        if let Ok(mut player) = players.get_mut(child) {
            player.play(anim.index).repeat();
            commands
                .entity(child)
                .insert(AnimationGraphHandle(anim.graph_handle.clone()));
            started += 1;
        }
        if let Ok(sm) = skinned.get(child) {
            skinned_meshes += 1;
            joints += sm.joints.len();
        }
    }
    // The demo claims LBS deformation — require a player to drive the take and
    // a `SkinnedMesh` with at least one joint (engine limits still apply:
    // LBS only, ≤256 joints, ≤4 weights/vertex).
    assert!(
        started > 0,
        "no AnimationPlayer under the {} scene root — loader must insert one",
        handles.path
    );
    assert!(
        skinned_meshes > 0 && joints > 0,
        "no SkinnedMesh with joints under the {} scene root — loader must insert one",
        handles.path
    );
    println!(
        "[skinned_mesh_fbx] scene ready: {skinned_meshes} SkinnedMesh entity(ies), \
         {joints} joint(s); playing take #{}",
        handles.anim_index
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
        Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.32, 0.42, 0.35))),
        Floor,
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

/// Fit the camera to the union world-space `Aabb` of all spawned meshes,
/// accumulated across [`FRAMING_SAMPLE_FRAMES`] frames so the wiggling skin
/// stays framed for the whole take. Falls back to the default framing on
/// timeout.
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
    // the floor plane and UI glyph meshes are not part of the content. The
    // loader's default `FbxSkinnedMeshBoundsPolicy::Dynamic` keeps the `Aabb`
    // updating from the joint transforms, so the union follows the deformation.
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
            "[skinned_mesh_fbx] no mesh bounds in the {}-frame sampling window — \
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

    // Keep the floor under the (possibly off-origin) content.
    if let Ok(mut floor) = floors.single_mut() {
        floor.translation = Vec3::new(center.x, framing.union_min.y - 0.02, center.z);
    }

    println!(
        "[skinned_mesh_fbx] content bounds: {} mesh AABB(s), union sampled over {} frames",
        framing.meshes,
        framing.frames
    );
    println!(
        "[skinned_mesh_fbx]   world min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4}) \
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
        "[skinned_mesh_fbx] camera framing: distance={distance:.4} v_fov={v_fov:.4}rad \
         aspect={aspect:.3} near={near:.4} translation=({:.4}, {:.4}, {:.4})",
        pos.x, pos.y, pos.z
    );
}

/// Live HUD: static clip facts once `Fbx` + `AnimationClip` finish loading
/// (take name from `Fbx::named_animations`), then joint count + live animation
/// seek time so the deformation claim is checkable frame to frame.
fn update_status(
    asset_server: Res<AssetServer>,
    handles: Res<DemoHandles>,
    fbxs: Res<Assets<Fbx>>,
    clips: Res<Assets<AnimationClip>>,
    skinned: Query<&SkinnedMesh>,
    players: Query<&AnimationPlayer>,
    mut texts: Query<&mut Text, With<StatusText>>,
    mut facts: Local<Option<String>>,
    mut printed: Local<bool>,
) {
    let mut lines = vec![format!("skinned_mesh_fbx — {}", handles.path)];
    lines.push("Look for: skinned mesh DEFORMING (joints / LBS), not a rigid body spin.".into());

    if facts.is_none() {
        let fbx_state = asset_server.load_state(&handles.fbx);
        let clip_state = asset_server.load_state(&handles.clip);
        if matches!(clip_state, LoadState::Failed(_)) {
            lines.push(format!(
                "Animation{} clip FAILED to load — see stderr",
                handles.anim_index
            ));
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
            let line = format!(
                "AnimationGraph plays take #{}/{} \"{take}\" → {label}: {:.3}s, {} take(s) total \
                 (no loader auto-play)",
                handles.anim_index,
                fbx.animations.len().saturating_sub(1),
                clip.duration(),
                fbx.animations.len(),
            );
            if !*printed {
                *printed = true;
                println!("[skinned_mesh_fbx] {line}");
            }
            *facts = Some(line);
        } else {
            lines.push("loading clip…".into());
        }
    }
    if let Some(line) = facts.as_ref() {
        lines.push(line.clone());
    }

    let (mut meshes_count, mut joints) = (0usize, 0usize);
    for sm in &skinned {
        meshes_count += 1;
        joints += sm.joints.len();
    }
    let (mut players_count, mut seek) = (0usize, 0.0);
    for player in &players {
        players_count += 1;
        for (_, active) in player.playing_animations() {
            seek = active.seek_time();
        }
    }
    lines.push(format!(
        "SkinnedMesh: {meshes_count} | joints: {joints} (LBS, engine caps 256) | \
         players: {players_count} | seek {seek:.2}s"
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
        // Translucent plate: the HUD stays legible over the white sausage mesh
        // (white-on-white overlap was a real user-reported bug class here).
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
