//! Nested mesh hierarchy (parent/child mesh nodes under one Scene0).
//!
//! ```sh
//! cargo run --example nested_meshes_fbx
//! ```
//!
//! Asset: `assets/blender_279_nested_meshes_7400_binary.fbx`
//! Look for: several distinct shapes (cube / cone / icosphere / plane) spinning
//! together as one hierarchy — not a single fused mesh. After spawn the
//! example prints the `FbxNode` asset tree (children nesting + per-node
//! `name` / `extras: Option<FbxExtras>` — `FbxNode.extras` in `src/types.rs`)
//! and counts `FbxExtras` components on the spawned entities, next to the
//! `Scene0` `Mesh3d` count. The camera auto-frames the measured content
//! `Aabb` (spin-invariant sphere around the Y axis).
//! Blender Auto → AdjustTransforms; demo scale 1.0.

use std::collections::HashSet;
use std::f32::consts::{FRAC_PI_4, PI};
use std::path::Path;

use bevy::{
    asset::LoadState,
    camera::primitives::Aabb,
    light::CascadeShadowConfigBuilder,
    prelude::*,
    window::PrimaryWindow,
    world_serialization::{WorldAssetRoot, WorldInstanceReady},
};
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxExtras, FbxNode, FbxPlugin};

const FBX_PATH: &str = "blender_279_nested_meshes_7400_binary.fbx";
const DEMO_VISUAL_SCALE: f32 = 1.0;

/// Frames to poll for mesh `Aabb`s before keeping the default framing.
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Frustum margin on the fitted distance (bounding sphere × slack).
const FRAMING_MARGIN: f32 = 1.15;
/// Viewpoint direction of the original hard-coded camera — the auto-fit
/// camera sits at the same diagonal angle, just at the measured distance.
const VIEW_DIR: Vec3 = Vec3::new(4.0, 3.0, 5.5);

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
        .init_resource::<CameraFraming>()
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate, report_hierarchy, frame_camera))
        .run();
}

#[derive(Component)]
struct Spinning;

#[derive(Component)]
struct ViewerCamera;

/// Demo ground plane — excluded from framing bounds, repositioned under the
/// measured content by `frame_camera`.
#[derive(Component)]
struct Ground;

#[derive(Component)]
struct StatusText;

/// Fit-to-content state machine (the `load_fbx` pattern): armed by the
/// `WorldInstanceReady` observer, polled until `Aabb`s exist.
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
}

/// The `Fbx` index load used to walk `FbxNode` assets + one-shot report flag.
#[derive(Resource)]
struct HierarchyReport {
    fbx: Handle<Fbx>,
    reported: bool,
}

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
        .observe(scene_ready);

    // Bare `Fbx` index load → `FbxNode` handles for the asset-side tree walk.
    commands.insert_resource(HierarchyReport {
        fbx: asset_server.load(FBX_PATH),
        reported: false,
    });

    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_translation(VIEW_DIR * 3.5).looking_at(Vec3::new(0.0, 0.3, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Ground,
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
             Waiting for Scene0 + FbxNode assets..."
        )),
        TextFont::from_font_size(16.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

/// Scene instance spawned → arm the fit-to-content camera framing.
fn scene_ready(_ready: On<WorldInstanceReady>, mut framing: ResMut<CameraFraming>) {
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
    }
}

/// Fit the camera to the measured content bounds (the `load_fbx` pattern),
/// using a **spin-invariant** bounding sphere around the Y axis through the
/// origin (the example spins the whole hierarchy). The ground plane is
/// re-fitted from the same measurement.
fn frame_camera(
    mut framing: ResMut<CameraFraming>,
    // Camera and ground both write `Transform`; `With<ViewerCamera>` /
    // `With<Ground>` do not prove disjointness (B0001), so they share a
    // ParamSet.
    mut rigs: ParamSet<(
        Query<(&mut Transform, &Projection), With<ViewerCamera>>,
        Query<&mut Transform, With<Ground>>,
    )>,
    meshes: Query<(&Aabb, &GlobalTransform), (With<Mesh3d>, Without<Ground>)>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !framing.armed || framing.done {
        return;
    }
    framing.frames += 1;

    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut corners: Vec<Vec3> = Vec::new();
    let mut count = 0usize;
    for (aabb, global) in meshes.iter() {
        let (lo, hi) = (aabb.min(), aabb.max());
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for z in [lo.z, hi.z] {
                    let world = global.transform_point(Vec3::new(x, y, z));
                    min = min.min(world);
                    max = max.max(world);
                    corners.push(world);
                }
            }
        }
        count += 1;
    }
    if count == 0 {
        if framing.frames >= FRAMING_TIMEOUT_FRAMES {
            framing.armed = false;
            framing.done = true;
            println!(
                "[nested_meshes_fbx] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
                 keeping default camera framing"
            );
        }
        return;
    }
    framing.armed = false;
    framing.done = true;

    let center = Vec3::new(0.0, (min.y + max.y) * 0.5, 0.0);
    let mut radius = (max.y - min.y).abs() * 0.5;
    for world in &corners {
        radius = radius.max((*world - center).length());
    }
    radius = radius.max(1e-4);

    {
        let mut ground_q = rigs.p1();
        if let Ok(mut ground) = ground_q.single_mut() {
            ground.translation.y = min.y - 0.06 * radius;
            ground.scale = Vec3::splat((radius * 0.6).max(1.0));
        }
    }

    let mut camera_q = rigs.p0();
    let Ok((mut cam_tf, projection)) = camera_q.single_mut() else {
        return;
    };
    let (v_fov, near) = match projection {
        Projection::Perspective(p) => (p.fov, p.near),
        _ => (FRAC_PI_4, 0.1),
    };
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

    println!(
        "[nested_meshes_fbx] content bounds: {count} mesh AABB(s) \
         min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4})",
        min.x, min.y, min.z, max.x, max.y, max.z
    );
    println!(
        "[nested_meshes_fbx] spin-invariant sphere center=({:.4}, {:.4}, {:.4}) \
         radius={:.4} camera distance={distance:.4}",
        center.x, center.y, center.z, radius
    );
}

/// One-shot report once `Scene0` bounds + the `Fbx` index are ready: count
/// spawned `Mesh3d`s, print the `FbxNode` asset tree (children nesting, per
/// node `name` + `extras`) and the authored-extras counts.
#[allow(clippy::too_many_arguments)]
fn report_hierarchy(
    mut report: ResMut<HierarchyReport>,
    framing: Res<CameraFraming>,
    asset_server: Res<AssetServer>,
    fbx_assets: Res<Assets<Fbx>>,
    fbx_nodes: Res<Assets<FbxNode>>,
    root: Query<Entity, With<WorldAssetRoot>>,
    children: Query<&Children>,
    mesh3d: Query<(), With<Mesh3d>>,
    extras: Query<(), With<FbxExtras>>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    if report.reported || !framing.done {
        return;
    }
    match asset_server.load_state(&report.fbx) {
        LoadState::Failed(err) => {
            report.reported = true;
            eprintln!("[nested_meshes_fbx] Fbx index load FAILED: {err}");
            if let Ok(mut text) = texts.single_mut() {
                *text = Text::new("nested_meshes_fbx — Fbx load FAILED\n(see stderr)");
            }
            return;
        }
        LoadState::Loaded => {}
        _ => return,
    }

    let Some(fbx) = fbx_assets.get(&report.fbx) else {
        return;
    };
    let Ok(root) = root.single() else {
        return;
    };

    // ── ECS side: spawned Scene0 hierarchy ──────────────────────────────────
    let mut mesh_count = 0usize;
    let mut extras_count = 0usize;
    for child in children.iter_descendants(root) {
        if mesh3d.get(child).is_ok() {
            mesh_count += 1;
        }
        if extras.get(child).is_ok() {
            extras_count += 1;
        }
    }

    // ── Asset side: FbxNode tree (children nesting + name/extras fields) ────
    let mut asset_extras = 0usize;
    for handle in fbx.nodes.iter() {
        if let Some(node) = fbx_nodes.get(handle)
            && node.extras.is_some()
        {
            asset_extras += 1;
        }
    }
    // Root nodes = never referenced as somebody's child (includes the
    // synthetic `<fbx-root>` when the file has one).
    let child_ids: HashSet<_> = fbx
        .nodes
        .iter()
        .flat_map(|n| {
            fbx_nodes
                .get(n)
                .map(|node| node.children.iter().map(|c| c.id()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();
    let mut tree = String::new();
    for handle in fbx.nodes.iter() {
        let Some(node) = fbx_nodes.get(handle) else {
            continue;
        };
        if child_ids.contains(&handle.id()) {
            continue;
        }
        if !tree.is_empty() {
            tree.push('\n');
            tree.push_str("  ");
        }
        write_node_tree(node, &fbx_nodes, &mut tree, 0);
    }

    println!("[nested_meshes_fbx] Scene0 Mesh3d count: {mesh_count} (dump expects 4)");
    println!("[nested_meshes_fbx] FbxNode tree:\n  {tree}");
    println!(
        "[nested_meshes_fbx] extras: FbxNode assets with `extras` = {asset_extras}/{}; \
         spawned FbxExtras components = {extras_count}",
        fbx.nodes.len()
    );

    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "nested_meshes_fbx — {FBX_PATH}\n\
             Look for: MULTIPLE shapes in one hierarchy (cube, cone, icosphere, plane).\n\
             FbxNode tree: {tree}\n\
             Scene0 Mesh3d={mesh_count} (dump expects 4) | extras: FbxNode {asset_extras}/{} | \
             spawned FbxExtras {extras_count}\n\
             Blender Auto -> AdjustTransforms; DEMO_VISUAL_SCALE={DEMO_VISUAL_SCALE}; \
             auto-framed to measured bounds.",
            fbx.nodes.len()
        ));
    }
    report.reported = true;
}

/// Depth-limited `name{child, child}` rendering of one `FbxNode` subtree.
fn write_node_tree(node: &FbxNode, all: &Assets<FbxNode>, out: &mut String, depth: usize) {
    const MAX_DEPTH: usize = 8;
    out.push_str(&node.name);
    if node.children.is_empty() || depth >= MAX_DEPTH {
        return;
    }
    out.push('{');
    for (i, child) in node.children.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match all.get(child) {
            Some(child_node) => write_node_tree(child_node, all, out, depth + 1),
            None => out.push('?'),
        }
    }
    out.push('}');
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.35);
    }
}
