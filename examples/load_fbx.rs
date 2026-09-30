//! Loads and displays an FBX file from ANY path.
//!
//! Pass a path as the first argument — either relative to the `assets/` directory
//! or an absolute filesystem path (spaces are fine, quote it in the shell):
//!
//! ```sh
//! cargo run --example load_fbx -- spider.fbx
//! cargo run --example load_fbx -- "C:\models\My Idle.fbx"
//! ```
//!
//! Absolute paths are *unapproved* for bevy_asset (they sit outside the `assets/`
//! folder), and Bevy 0.19's `AssetPlugin::unapproved_path_mode` defaults to
//! [`UnapprovedPathMode::Forbid`](bevy::asset::UnapprovedPathMode), which rejects
//! them **before the loader ever runs**. This local dev-tool example therefore
//! opts in app-wide with `UnapprovedPathMode::Allow`. The Bevy docs "strongly
//! discourage" `Allow` for apps with scripts or modding support — do NOT copy
//! this escape hatch into a shipping game (use `LoadBuilder::override_unapproved()`
//! for one-off loads there instead). Direct exe runs (bypassing `cargo run`) need
//! `BEVY_ASSET_ROOT` for `assets/`-relative paths; absolute paths need no env at all.
//!
//! The loaded model spins slowly so you can inspect it from all sides. After the
//! scene spawns, the camera auto-frames the union `Aabb` of the spawned meshes
//! (bounds + final camera transform are printed to stdout), so cm-scaled Mixamo
//! characters and tiny fixtures both land on screen. If loading fails the granular
//! `FbxError` is printed to stderr and on-screen — never a silent gray screen.
//!
//! Set `FBX_SETTINGS=1` to run the same load through
//! `AssetServer::load_builder().with_settings(...)` with `FbxLoaderSettings`
//! (see the `use_settings` branch in `setup`). Sampler-tier settings
//! (`default_sampler` / `override_sampler`) are demonstrated in
//! `examples/sampler_settings_fbx.rs`.

use std::f32::consts::FRAC_PI_4;

use bevy::asset::{AssetLoadError, AssetPlugin, LoadState, UnapprovedPathMode};
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldInstanceReady};
use bevy_ufbx::{FbxError, FbxLoaderSettings, FbxPlugin};

/// Frames to poll for mesh `Aabb`s before keeping the default camera framing
/// (`Aabb` is inserted one frame after the scene entities spawn).
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Viewpoint direction of the original hard-coded camera — kept so the auto-fit
/// camera sits at the same diagonal angle, just at the right distance.
const VIEW_DIR: Vec3 = Vec3::new(4.0, 4.0, 8.0);
/// Frustum margin on the fitted distance (fits the bounding sphere, ×1.15 slack).
const FRAMING_MARGIN: f32 = 1.15;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            // Dev-tool escape hatch: `load_fbx` is a "load any file" viewer, so it
            // accepts absolute paths. Bevy's default `UnapprovedPathMode::Forbid`
            // rejects them BEFORE the loader runs (gray screen + "path is unapproved"
            // error). See the module docs for the Bevy-documented caveat on `Allow`.
            unapproved_path_mode: UnapprovedPathMode::Allow,
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .init_resource::<CameraFraming>()
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate, watch_load, frame_camera_when_ready))
        .run();
}

// ── Components / resources ──────────────────────────────────────────────────

#[derive(Component)]
struct Spinning;

#[derive(Component)]
struct ViewerCamera;

#[derive(Component)]
struct StatusText;

/// The `Scene0` handle being loaded + one-shot terminal-state reporting.
#[derive(Resource)]
struct LoadWatch {
    handle: Handle<WorldAsset>,
    path: String,
    resolved: bool,
}

/// Fit-to-content state machine: armed by the `WorldInstanceReady` observer,
/// polled until `Aabb`s exist (or the timeout keeps the default framing).
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
}

// ── Setup ───────────────────────────────────────────────────────────────────

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // ── Camera (framing system repositions it once content bounds exist) ─────
    // near=0.01 (Bevy default is 0.1) lets the fit-to-content framing get close
    // to tiny models — Bevy's reverse-infinite-Z projection keeps depth precision
    // fine at this near/far ratio.
    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_xyz(4.0, 4.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // ── Lighting ─────────────────────────────────────────────────────────────
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(
            EulerRot::XYZ,
            -45_f32.to_radians(),
            45_f32.to_radians(),
            0.0,
        )),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 200.0,
        affects_lightmapped_meshes: false,
    });

    // ── FBX path from CLI args (defaults to "cube.fbx") ──────────────────────
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cube.fbx".to_string());

    // --- Load Scene0: default settings, or FbxLoaderSettings when FBX_SETTINGS=1 ---
    // Current API: `load_builder().with_settings(...)` (the old
    // `load_with_settings` is deprecated). Sampler-tier settings live in
    // `examples/sampler_settings_fbx.rs`; this branch shows import-shape knobs.
    let use_settings = std::env::var_os("FBX_SETTINGS").is_some();
    let scene: Handle<WorldAsset> = if use_settings {
        asset_server
            .load_builder()
            .with_settings(|settings: &mut FbxLoaderSettings| {
                // Full knob list on `FbxLoaderSettings`: load_meshes /
                // load_materials / load_animations / bake_fps /
                // generate_rest_animation / include_source / convert_coordinates /
                // space_conversion / skinned_mesh_bounds_policy / default_sampler /
                // override_sampler …
                settings.load_cameras = false;
                settings.load_lights = false;
            })
            .load(format!("{path}#Scene0"))
    } else {
        asset_server.load(format!("{path}#Scene0"))
    };

    commands
        .spawn((WorldAssetRoot(scene.clone()), Spinning))
        .observe(scene_ready);

    commands.insert_resource(LoadWatch {
        handle: scene,
        path: path.clone(),
        resolved: false,
    });

    commands.spawn((
        StatusText,
        // On-screen text uses ASCII only: Bevy's default font (FiraMono-subset)
        // lacks glyphs for "—", "…" and "×" (they render as tofu boxes).
        Text::new(format!("load_fbx - {path}\nLoading Scene0...")),
        TextFont::from_font_size(16.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));

    if use_settings {
        info!("Loading '{path}' with FbxLoaderSettings (load_cameras=false, load_lights=false) …");
    } else {
        info!("Loading '{path}' … (FBX_SETTINGS=1 to demo FbxLoaderSettings)");
    }
}

// ── Systems ──────────────────────────────────────────────────────────────────

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}

/// Scene instance spawned → arm the fit-to-content camera framing.
fn scene_ready(_ready: On<WorldInstanceReady>, mut framing: ResMut<CameraFraming>) {
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
    }
    info!("Scene0 instance spawned — fitting camera to content bounds …");
}

/// Watch the `Scene0` load: report success once, print the granular `FbxError`
/// to stderr (plus an on-screen message) on failure — never a silent gray screen.
fn watch_load(
    asset_server: Res<AssetServer>,
    mut watch: ResMut<LoadWatch>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    if watch.resolved {
        return;
    }
    match asset_server.load_state(&watch.handle) {
        LoadState::Loaded => {
            watch.resolved = true;
            println!("[load_fbx] Scene0 loaded OK: {}", watch.path);
            if let Ok(mut text) = texts.single_mut() {
                *text = Text::new(format!(
                    "load_fbx - {}\nScene0 loaded - auto-framing content bounds...",
                    watch.path
                ));
            }
        }
        LoadState::Failed(err) => {
            watch.resolved = true;
            print_load_error(&watch.path, &err);
            if let Ok(mut text) = texts.single_mut() {
                *text = Text::new(format!(
                    "load_fbx - FAILED to load {}\n(see stderr for the granular FbxError)",
                    watch.path
                ));
            }
        }
        _ => {}
    }
}

/// Fit the camera to the union world-space `Aabb` of all spawned meshes.
/// Falls back to the default framing if no bounds appear within the timeout.
fn frame_camera_when_ready(
    mut framing: ResMut<CameraFraming>,
    mut camera: Query<(&mut Transform, &Projection), With<ViewerCamera>>,
    meshes: Query<(&Aabb, &GlobalTransform), With<Mesh3d>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !framing.armed || framing.done {
        return;
    }
    framing.frames += 1;

    // Union of world-space mesh bounds. `Aabb` is inserted by Bevy's
    // calculate_bounds after spawn, so poll until at least one exists.
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut count = 0usize;
    for (aabb, global) in meshes.iter() {
        let lo = aabb.min();
        let hi = aabb.max();
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for z in [lo.z, hi.z] {
                    let world = global.transform_point(Vec3::new(x, y, z));
                    min = min.min(world);
                    max = max.max(world);
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
                "[load_fbx] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
                 keeping default camera framing"
            );
        }
        return;
    }
    framing.armed = false;
    framing.done = true;

    let center = (min + max) * 0.5;
    let radius = ((max - min) * 0.5).length().max(1e-5);

    let Ok((mut cam_tf, projection)) = camera.single_mut() else {
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
    // Keep the model outside the near plane for tiny content (near defaults to 0.1).
    distance = distance.max(near * 1.5 + radius);

    let pos = center + VIEW_DIR.normalize() * distance;
    *cam_tf = Transform::from_translation(pos).looking_at(center, Vec3::Y);

    println!("[load_fbx] content bounds: {count} mesh AABB(s)");
    println!(
        "[load_fbx]   world min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4}) \
         center=({:.4}, {:.4}, {:.4}) radius={:.4}",
        min.x, min.y, min.z, max.x, max.y, max.z, center.x, center.y, center.z, radius
    );
    println!(
        "[load_fbx] camera framing: distance={distance:.4} v_fov={v_fov:.4}rad \
         aspect={aspect:.3} near={near:.4}"
    );
    println!(
        "[load_fbx]   translation=({:.4}, {:.4}, {:.4}) looking_at center=({:.4}, {:.4}, {:.4})",
        pos.x, pos.y, pos.z, center.x, center.y, center.z
    );
}

// ── Error display (granular FbxError on load failure — mirrors dump_fbx) ────

fn print_load_error(path: &str, err: &AssetLoadError) {
    eprintln!("=== load_fbx: failed to load '{path}' ===");

    let AssetLoadError::AssetLoaderError(loader_err) = err else {
        eprintln!("  AssetLoadError (non-loader failure): {err}");
        return;
    };
    let Some(fbx_err) = loader_err.error().downcast_ref::<FbxError>() else {
        eprintln!(
            "  AssetLoaderError for '{}' (not an FbxError): {}",
            loader_err.path(),
            loader_err.error()
        );
        return;
    };

    eprintln!("  FbxError (Debug): {fbx_err:?}");
    match fbx_err {
        FbxError::Io(source) => {
            eprintln!("    Io: {source}");
        }
        FbxError::UfbxError(message) => {
            eprintln!("    UfbxError: {message}");
        }
        FbxError::UfbxLoad { path, message } => {
            eprintln!("    UfbxLoad: path={path:?} message={message:?}");
        }
        FbxError::InvalidData(message) => {
            eprintln!("    InvalidData: {message:?}");
        }
        FbxError::ConversionError(message) => {
            eprintln!("    ConversionError: {message:?}");
        }
        FbxError::MeshConversion(message) => {
            eprintln!("    MeshConversion: {message:?}");
        }
        FbxError::MorphTargets { mesh, message } => {
            eprintln!("    MorphTargets: mesh={mesh:?} message={message:?}");
        }
        FbxError::MaterialConversion(message) => {
            eprintln!("    MaterialConversion: {message:?}");
        }
        FbxError::TextureLoad(message) => {
            eprintln!("    TextureLoad: {message:?}");
        }
        FbxError::UnsupportedFeature(message) => {
            eprintln!("    UnsupportedFeature: {message:?}");
        }
        other => {
            // `FbxError` is `#[non_exhaustive]`: future variants land here.
            eprintln!("    (other FbxError variant): {other}");
        }
    }
}
