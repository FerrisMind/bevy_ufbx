//! Side-by-side visual coverage checklist for native FBX features.
//!
//! ```sh
//! cargo run --example showcase_fbx
//! ```
//!
//! Spawns multiple `WorldAssetRoot`s: morph, TRS anim, NURBS, skin, multimaterial.
//! Look for: five labeled demos in one window (coverage checklist). The camera
//! fits the measured union `Aabb` of the spawned slots (fit-to-content, bounds
//! printed to stdout), and the name plates are projected onto a bottom strip of
//! the fixed 1600x900 window so neighbours can never overlap. Animated slots
//! start playback in a `WorldInstanceReady` observer — the example drives
//! `AnimationPlayer` itself; the loader never auto-plays. After framing settles,
//! every slot reports its runtime contents (meshes / morph / skin / players /
//! materials) to stdout and the HUD, so each claimed feature is verified live.

use std::collections::HashSet;
use std::f32::consts::{FRAC_PI_4, PI};

use bevy::camera::primitives::Aabb;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};
use bevy::mesh::morph::MorphWeights;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::world_serialization::WorldInstanceReady;
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const MORPH: &str = "blend_shape_cube.fbx";
const ANIM: &str = "cube_anim.fbx";
const NURBS: &str = "nurbs_saddle.fbx";
const SKIN: &str = "blender_279_sausage_7400_binary.fbx";
const SKIN_FALLBACK: &str = "rigged_triangle.fbx";
const MULTI: &str = "blender_suzanne_multimaterial_7400_binary.fbx";

const CM_SCALE: f32 = 80.0;

/// Frames to poll for mesh `Aabb`s before keeping the default camera framing
/// (`Aabb` is inserted one frame after the scene entities spawn).
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Front-elevated viewpoint — keeps the left-to-right slot order stable on
/// screen so the projected name plates line up under their slots.
const VIEW_DIR: Vec3 = Vec3::new(0.0, 0.32, 1.0);
/// Frustum margin on the fitted distance (slack for the animated slots).
const FRAMING_MARGIN: f32 = 1.2;
/// Minimum horizontal gap between neighbouring name plates (px). Widest label
/// ("multi-mat", 9 chars at font 26) is ~140 px; the widest adjacent pair
/// (half-widths + 24 px slack) needs ~125 px, so 140 px guarantees no overlap
/// while keeping the plates close to their projected columns.
const PLATE_GAP: f32 = 140.0;

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
                // Fixed window: the projected name-plate pixel positions below
                // are deterministic (repo pattern: sampler_settings_fbx).
                resolution: (1600u32, 900u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .init_resource::<CameraFraming>()
        .init_resource::<Slots>()
        .add_systems(Startup, setup)
        .add_systems(Update, (frame_camera, report_slots).chain())
        .run();
}

#[derive(Component)]
struct AnimationToPlay {
    graph_handle: Handle<AnimationGraph>,
    index: AnimationNodeIndex,
}

#[derive(Component)]
struct ViewerCamera;

/// Demo ground plane — excluded from framing bounds, repositioned under the
/// measured content by `frame_camera`.
#[derive(Component)]
struct Ground;

#[derive(Component)]
struct StatusText;

/// Fit-to-content state machine: armed by the `WorldInstanceReady` observers,
/// polled until every slot spawned and all `Aabb`s exist (or the timeout keeps
/// the default framing).
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
}

/// One spawned demo slot (label, fixture, root entity, animation clip index).
struct Slot {
    label: &'static str,
    path: &'static str,
    root: Entity,
}

/// Slot registry + one-shot runtime report state.
#[derive(Resource, Default)]
struct Slots {
    entries: Vec<Slot>,
    /// Slots skipped because their fixture is absent: (label, fixture).
    skipped: Vec<(&'static str, &'static str)>,
    /// `WorldInstanceReady` observers fired so far.
    spawned: usize,
    /// Slots that were spawned (expected `WorldInstanceReady` count).
    expected: usize,
    reported: bool,
}

const HUD_BASE: &str = "showcase_fbx - visual coverage checklist\n\
    L->R: morph | anim TRS | nurbs | skin | multi-mat\n\
    Each slot is its own WorldAssetRoot (Scene0). Animations auto-start via a\n\
    WorldInstanceReady observer (this example - the loader never auto-plays).";

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut slots: ResMut<Slots>,
) {
    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        // Framing system repositions the camera once content bounds exist;
        // this is the fallback view for the timeout path.
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
        Ground,
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

    // Five slots L->R, each its own WorldAssetRoot. `slot_ready` counts
    // instances for the framing/report state machine; animated slots also get
    // `play_when_ready` to start their clip (the loader never auto-plays).
    let root = spawn_animated(
        &mut commands,
        &asset_server,
        &mut graphs,
        MORPH,
        0,
        Vec3::new(-5.5, 0.0, 0.0),
        CM_SCALE,
    );
    slots.entries.push(Slot {
        label: "morph",
        path: MORPH,
        root,
    });

    let root = spawn_animated(
        &mut commands,
        &asset_server,
        &mut graphs,
        ANIM,
        0,
        Vec3::new(-2.75, 0.0, 0.0),
        CM_SCALE,
    );
    slots.entries.push(Slot {
        label: "anim TRS",
        path: ANIM,
        root,
    });

    let nurbs_root = commands
        .spawn((
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(NURBS))),
            Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)).with_scale(Vec3::splat(CM_SCALE)),
        ))
        .observe(slot_ready)
        .id();
    slots.entries.push(Slot {
        label: "nurbs",
        path: NURBS,
        root: nurbs_root,
    });

    // Rigged fixture with a metre-ish fallback (its Aabb is 0..1, so the demo
    // scale must NOT be the cm-style CM_SCALE — that would birth an 80-unit
    // triangle and blow the framing apart).
    let (skin_path, skin_scale, skin_anim) = if std::path::Path::new("assets").join(SKIN).is_file()
    {
        (SKIN, 0.85, 2usize)
    } else {
        (SKIN_FALLBACK, 1.5, 0usize)
    };
    let root = spawn_animated(
        &mut commands,
        &asset_server,
        &mut graphs,
        skin_path,
        skin_anim,
        Vec3::new(2.75, 0.0, 0.0),
        skin_scale,
    );
    slots.entries.push(Slot {
        label: "skin",
        path: skin_path,
        root,
    });

    // Blender multimaterial Suzanne (AdjustTransforms -> metre-ish). If the
    // fixture is absent the slot is skipped for real: no root, no plate, and
    // the report says SKIPPED instead of claiming a slot that never spawned.
    if std::path::Path::new("assets").join(MULTI).is_file() {
        let root = commands
            .spawn((
                WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(MULTI))),
                Transform::from_translation(Vec3::new(5.5, 0.0, 0.0)).with_scale(Vec3::splat(0.9)),
            ))
            .observe(slot_ready)
            .id();
        slots.entries.push(Slot {
            label: "multi-mat",
            path: MULTI,
            root,
        });
    } else {
        slots.skipped.push(("multi-mat", MULTI));
    }

    slots.expected = slots.entries.len();

    commands.spawn((
        StatusText,
        Text::new(format!(
            "{HUD_BASE}\nwaiting for {} slots + mesh bounds...",
            slots.expected
        )),
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
) -> Entity {
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
        .observe(play_when_ready)
        .observe(slot_ready)
        .id()
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

/// One more `WorldInstanceReady` observed: count it and arm the fit-to-content
/// camera framing (re-armed until every slot has reported in).
fn slot_ready(
    _ready: On<WorldInstanceReady>,
    mut slots: ResMut<Slots>,
    mut framing: ResMut<CameraFraming>,
) {
    slots.spawned += 1;
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
    }
}

/// Projects `p` to NDC for a camera at `pos` with an orthonormal basis
/// (`fwd` = view direction). `None` when the point is behind the camera.
fn project_ndc(
    p: Vec3,
    pos: Vec3,
    right: Vec3,
    up: Vec3,
    fwd: Vec3,
    tan_h: f32,
    tan_v: f32,
) -> Option<Vec2> {
    let v = p - pos;
    let z = v.dot(fwd);
    (z > 0.0).then(|| Vec2::new(v.dot(right) / (z * tan_h), v.dot(up) / (z * tan_v)))
}

/// Fit-to-content pass: seat every slot's measured bounds on the ground plane,
/// fit the camera to the union `Aabb`, refit the shadow cascades, and spawn the
/// name plates at their slots' projected screen positions (bottom strip, fixed
/// window, monotonic 170 px gaps -> neighbours cannot overlap). Every decision
/// is printed so a headless/timeout run can verify the framing from stdout.
#[allow(clippy::too_many_arguments)]
fn frame_camera(
    mut commands: Commands,
    mut framing: ResMut<CameraFraming>,
    slots: Res<Slots>,
    // Camera, ground and slot roots all write `Transform`; the query filters
    // do not prove disjointness (B0001), so they share a ParamSet.
    mut rigs: ParamSet<(
        Query<(&mut Transform, &Projection), With<ViewerCamera>>,
        Query<&mut Transform, With<Ground>>,
        Query<&mut Transform, With<WorldAssetRoot>>,
    )>,
    meshes: Query<(&Aabb, &GlobalTransform), (With<Mesh3d>, Without<Ground>)>,
    pending: Query<(), (With<Mesh3d>, Without<Aabb>, Without<Ground>)>,
    children: Query<&Children>,
    mut cascades: Query<&mut CascadeShadowConfig, With<DirectionalLight>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !framing.armed || framing.done {
        return;
    }
    framing.frames += 1;
    let timed_out = framing.frames >= FRAMING_TIMEOUT_FRAMES;

    // Wait until every slot instance exists (a fixture that never loads trips
    // the timeout below instead of hanging the demo).
    if slots.spawned < slots.expected && !timed_out {
        return;
    }
    if slots.spawned < slots.expected {
        println!(
            "[showcase] only {}/{} slots ready after {FRAMING_TIMEOUT_FRAMES} frames — \
             framing what exists",
            slots.spawned, slots.expected
        );
    }
    // `Aabb` is inserted one frame after the scene entities spawn — poll until
    // every mesh has one (or the timeout proceeds with what landed).
    if !pending.is_empty() && !timed_out {
        return;
    }

    // Per-slot world bounds (ground excluded: it is dressing, not content).
    struct SlotBounds {
        min: Vec3,
        max: Vec3,
        /// Y shift that seats the slot's lowest corner on the ground plane.
        delta: f32,
        meshes: usize,
    }
    let mut per_slot: Vec<Option<SlotBounds>> = Vec::with_capacity(slots.entries.len());
    for entry in &slots.entries {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut n = 0usize;
        for child in children.iter_descendants(entry.root) {
            if let Ok((aabb, global)) = meshes.get(child) {
                let (lo, hi) = (aabb.min(), aabb.max());
                for x in [lo.x, hi.x] {
                    for y in [lo.y, hi.y] {
                        for z in [lo.z, hi.z] {
                            let world = global.transform_point(Vec3::new(x, y, z));
                            min = min.min(world);
                            max = max.max(world);
                        }
                    }
                }
                n += 1;
            }
        }
        per_slot.push((n > 0).then_some(SlotBounds {
            min,
            max,
            delta: -min.y,
            meshes: n,
        }));
    }

    // Union of the seated (delta-applied) slot boxes. Slots do not spin at the
    // root, so a plain bounding sphere around the union center is enough.
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut count = 0usize;
    for bounds in per_slot.iter().flatten() {
        let d = Vec3::new(0.0, bounds.delta, 0.0);
        for x in [bounds.min.x, bounds.max.x] {
            for y in [bounds.min.y, bounds.max.y] {
                for z in [bounds.min.z, bounds.max.z] {
                    let c = Vec3::new(x, y, z) + d;
                    min = min.min(c);
                    max = max.max(c);
                }
            }
        }
        count += bounds.meshes;
    }
    if count == 0 {
        if !timed_out {
            return;
        }
        // No content at all: keep the fallback camera and still place the
        // plates (evenly spaced) so the checklist labels stay visible.
        framing.armed = false;
        framing.done = true;
        println!(
            "[showcase] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
             keeping default camera framing"
        );
        if let Ok(window) = windows.single() {
            let plate_y = -(window.resolution.height() as f32) * 0.5 + 72.0;
            let n = slots.entries.len() as f32;
            for (i, entry) in slots.entries.iter().enumerate() {
                let px = (i as f32 - (n - 1.0) * 0.5) * 210.0;
                spawn_plate(&mut commands, entry.label, px, plate_y);
                println!(
                    "[showcase] plate '{}' (fallback): screen=({px:.0},{plate_y:.0}) px",
                    entry.label
                );
            }
        }
        return;
    }
    framing.armed = false;
    framing.done = true;

    // Seat each slot's lowest corner on y = 0 (the ground rests just below).
    {
        let mut roots_q = rigs.p2();
        for (entry, bounds) in slots.entries.iter().zip(&per_slot) {
            if let Some(bounds) = bounds
                && bounds.delta != 0.0
                && let Ok(mut root_tf) = roots_q.get_mut(entry.root)
            {
                root_tf.translation.y += bounds.delta;
            }
        }
    }

    let center = (min + max) * 0.5;
    let mut radius = (max.y - min.y).abs() * 0.5;
    for x in [min.x, max.x] {
        for y in [min.y, max.y] {
            for z in [min.z, max.z] {
                radius = radius.max(Vec3::new(x, y, z).distance(center));
            }
        }
    }
    radius = radius.max(1e-4);

    {
        let mut ground_q = rigs.p1();
        if let Ok(mut ground) = ground_q.single_mut() {
            ground.translation.y = min.y - 0.04;
            ground.scale = Vec3::splat((radius * 0.4).max(1.0));
        }
    }
    if let Ok(mut cascade) = cascades.single_mut() {
        *cascade = CascadeShadowConfigBuilder {
            first_cascade_far_bound: (radius * 4.0).max(1.0),
            maximum_distance: (radius * 8.0).max(2.0),
            ..default()
        }
        .build();
    }

    let aspect = windows
        .single()
        .map(|w| {
            let h = w.resolution.height();
            if h > 0.0 {
                w.resolution.width() as f32 / h as f32
            } else {
                1.0
            }
        })
        .unwrap_or(1.0)
        .max(0.1);
    let (half_w, half_h) = windows
        .single()
        .map(|w| {
            (
                w.resolution.width() as f32 * 0.5,
                w.resolution.height() as f32 * 0.5,
            )
        })
        .unwrap_or((800.0, 450.0));

    let (pos, v_fov) = {
        let mut camera_q = rigs.p0();
        let Ok((mut cam_tf, projection)) = camera_q.single_mut() else {
            return;
        };
        let (v_fov, near) = match projection {
            Projection::Perspective(p) => (p.fov, p.near),
            _ => (FRAC_PI_4, 0.1),
        };
        let h_fov = 2.0 * ((0.5 * v_fov).tan() * aspect).atan();
        let half_angle = 0.5 * v_fov.min(h_fov);
        let mut distance = radius / half_angle.sin() * FRAMING_MARGIN;
        // Keep the content outside the near plane for tiny fixtures.
        distance = distance.max(near * 1.5 + radius);

        let pos = center + VIEW_DIR.normalize() * distance;
        *cam_tf = Transform::from_translation(pos).looking_at(center, Vec3::Y);

        println!(
            "[showcase] content bounds: {count} mesh AABB(s) in {} slots",
            { per_slot.iter().filter(|b| b.is_some()).count() }
        );
        println!(
            "[showcase]   seated slots: world min=({:.4}, {:.4}, {:.4}) \
             max=({:.4}, {:.4}, {:.4}) center=({:.4}, {:.4}, {:.4}) radius={:.4}",
            min.x, min.y, min.z, max.x, max.y, max.z, center.x, center.y, center.z, radius
        );
        println!(
            "[showcase] camera framing: distance={distance:.4} v_fov={v_fov:.4}rad \
             aspect={aspect:.3} near={near:.4} translation=({:.4}, {:.4}, {:.4})",
            pos.x, pos.y, pos.z
        );
        (pos, v_fov)
    };

    // ── Name plates: X = the slot's ground column (its measured center at
    // y=0, z=0 — depth-independent so plates sit under the slot, not skewed
    // by content sticking toward the camera), Y = a bottom strip (the HUD
    // owns the top). A min-displacement two-pass layout keeps neighbours at
    // least PLATE_GAP apart while moving plates as little as possible, and
    // the fixed 1600x900 window makes the pixel positions deterministic.
    let plate_y = -half_h + 72.0;
    let fwd = (center - pos).normalize();
    let right = fwd.cross(Vec3::Y).normalize();
    let up = right.cross(fwd);
    let tan_v = (0.5 * v_fov).tan();
    let tan_h = tan_v * aspect;
    let edge = half_w - PLATE_GAP;
    let mut placed: Vec<(usize, f32)> = Vec::new();
    for i in 0..slots.entries.len() {
        let Some(bounds) = &per_slot[i] else {
            continue; // slot never produced bounds — no plate to place
        };
        let column = Vec3::new((bounds.min.x + bounds.max.x) * 0.5, 0.0, 0.0);
        let ndc_x = project_ndc(column, pos, right, up, fwd, tan_h, tan_v)
            .map(|n| n.x)
            .unwrap_or(0.0);
        placed.push((i, (ndc_x * half_w).clamp(-edge, edge)));
    }
    // Min-displacement layout: the forward pass pushes only pairs that are
    // actually closer than PLATE_GAP; the backward pass gives the drift back
    // toward the conflict, so plates that already clear PLATE_GAP stay on
    // their projected columns. Total needed span (4 x 140 px) fits the 1320 px
    // strip, so the constraint is always satisfiable within the edge clamps.
    let mut prev = f32::NEG_INFINITY;
    for p in &mut placed {
        p.1 = (p.1.max(prev + PLATE_GAP)).min(edge);
        prev = p.1;
    }
    let mut next = f32::INFINITY;
    for p in placed.iter_mut().rev() {
        p.1 = (p.1.min(next - PLATE_GAP)).max(-edge);
        next = p.1;
    }
    for (i, px) in placed {
        let entry = &slots.entries[i];
        spawn_plate(&mut commands, entry.label, px, plate_y);
        println!(
            "[showcase] plate '{}' ({}): screen=({px:.0},{plate_y:.0}) px",
            entry.label, entry.path
        );
    }
}

fn spawn_plate(commands: &mut Commands, label: &str, x: f32, y: f32) {
    commands.spawn((
        Text2d::new(label),
        TextFont::from_font_size(26.0),
        TextColor(Color::srgb(1.0, 0.95, 0.7)),
        TextLayout::justify(Justify::Center),
        Transform::from_xyz(x, y, 0.0),
    ));
}

/// One-shot runtime report: after framing settles, walk each slot's subtree and
/// print (and show on the HUD) what actually spawned — meshes, morph weights,
/// skins, players and distinct materials — so the checklist claims are verified
/// live instead of asserted in the header.
#[allow(clippy::too_many_arguments)]
fn report_slots(
    mut slots: ResMut<Slots>,
    framing: Res<CameraFraming>,
    children: Query<&Children>,
    mesh3d: Query<&Mesh3d>,
    morphs: Query<&MorphWeights>,
    skinned: Query<&SkinnedMesh>,
    players: Query<&AnimationPlayer>,
    materials: Query<&MeshMaterial3d<StandardMaterial>>,
    to_play: Query<&AnimationToPlay>,
    all_meshes: Res<Assets<Mesh>>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    if !framing.done || slots.reported {
        return;
    }
    slots.reported = true;

    println!("[showcase] slot report (runtime contents under each WorldAssetRoot):");
    let mut hud = format!(
        "{HUD_BASE}\nslots: {}/{} spawned - per-slot contents below and on stdout",
        slots.entries.len(),
        slots.expected
    );
    for entry in &slots.entries {
        let mut mesh_count = 0usize;
        let mut verts = 0usize;
        let mut morph_count = 0usize;
        let mut skinned_count = 0usize;
        let mut joints = 0usize;
        let mut player_count = 0usize;
        let mut playing = 0usize;
        let mut mat_ids = HashSet::new();
        let anim_index = to_play.get(entry.root).ok().map(|a| a.index);
        for child in children.iter_descendants(entry.root) {
            if let Ok(mesh) = mesh3d.get(child) {
                mesh_count += 1;
                if let Some(data) = all_meshes.get(&mesh.0) {
                    verts += data.count_vertices();
                }
            }
            if morphs.get(child).is_ok() {
                morph_count += 1;
            }
            if let Ok(skin) = skinned.get(child) {
                skinned_count += 1;
                joints = joints.max(skin.joints.len());
            }
            if let Ok(mat) = materials.get(child) {
                mat_ids.insert(mat.0.id());
            }
            if let Ok(player) = players.get(child) {
                player_count += 1;
                if let Some(index) = anim_index
                    && player.is_playing_animation(index)
                {
                    playing += 1;
                }
            }
        }
        println!(
            "  [{}] {}: meshes={mesh_count} verts={verts} morph={morph_count} \
             skinned={skinned_count} max_joints={joints} materials={} \
             players={player_count} playing={playing}",
            entry.label,
            entry.path,
            mat_ids.len()
        );
        hud.push_str(&format!(
            "\n{}: meshes={mesh_count} morph={morph_count} skinned={skinned_count} \
             players={playing}/{player_count}",
            entry.label
        ));
    }
    for (label, path) in &slots.skipped {
        println!("  [{label}] SKIPPED: assets/{path} not found — no root, no plate");
        hud.push_str(&format!("\n{label}: SKIPPED (fixture missing)"));
    }
    println!("[showcase] -------------------------------------------");

    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(hud);
    }
}
