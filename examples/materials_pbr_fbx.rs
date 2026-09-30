//! PBR scalar parity: specular reflectance, anisotropy, clearcoat.
//!
//! ```sh
//! cargo run --example materials_pbr_fbx
//! cargo run --example materials_pbr_fbx -- cube.fbx    # any file under assets/
//! cargo run --example materials_pbr_fbx --features pbr_multi_layer_material_textures
//! ```
//!
//! What this demonstrates (feature → where to look):
//! - specular strength : ufbx `specular_factor × 0.5` → `StandardMaterial.reflectance`
//!   (`src/material.rs::apply_specular`). Startup stdout prints the authored flags from
//!   an in-example `ufbx` probe (loader-matching opts); after load the resulting
//!   `reflectance` is printed from `Assets<StandardMaterial>`.
//! - specular tint     : ufbx `specular_color` → `StandardMaterial.specular_tint`.
//! - anisotropy        : `specular_anisotropy` / `specular_rotation` → `anisotropy_strength`
//!   / `anisotropy_rotation`, applied only when the file authors them (Bevy 0.19 keeps
//!   these scalars unconditional, so unauthored → Bevy default 0).
//! - clearcoat scalars : `coat_factor` / `coat_roughness` → `clearcoat` /
//!   `clearcoat_perceptual_roughness`, gated on ufbx's `coat` feature flag.
//! - coat TEXTURES     : only with `--features pbr_multi_layer_material_textures` —
//!   `StandardMaterial.clearcoat_*_texture` fields compile out without it, so the
//!   example prints which variant it was built with (and a note when compiled without).
//! - compressed formats: `FbxCompressedImageFormatSupport` printed at startup — the
//!   loader skips DDS/KTX2 containers the render device cannot decode.
//!
//! Default fixture `blender_279_internal_textures_7400_binary.fbx` authors
//! `specular_factor = 0.25` → reflectance 0.125 (distinct from Bevy's 0.5 default), so
//! the parity is visible in stdout. The camera fits the measured content bounds
//! (fit-to-content, printed to stdout), so any fixture passed on the CLI lands
//! on screen at its loader scale — no hand-tuned demo scale.

use std::f32::consts::{FRAC_PI_4, PI};
use std::path::Path;

use bevy::asset::LoadState;
use bevy::camera::primitives::Aabb;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::world_serialization::WorldInstanceReady;
use bevy_ufbx::{FbxAssetLabel, FbxCompressedImageFormatSupport, FbxPlugin};

const DEFAULT_FIXTURE: &str = "blender_279_internal_textures_7400_binary.fbx";
/// Loader conversion puts the default fixture at metre-ish extents (fixture
/// Aabb ±1.0 at scale 1.0) — the fit-to-content camera frames whatever loads,
/// so the demo scale stays 1.0 for every file. (The old ×100 "cm fixture"
/// assumption put the camera inside a 200-unit cube: backface culling then
/// hides the model — the bug class `sampler_settings_fbx` documents.)
const DEMO_VISUAL_SCALE: f32 = 1.0;

/// Frames to poll for mesh `Aabb`s before keeping the default camera framing
/// (`Aabb` is inserted one frame after the scene entities spawn).
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Direction of the original hard-coded camera offset — the auto-fit camera
/// keeps this diagonal angle and derives the distance from the measured bounds.
const VIEW_DIR: Vec3 = Vec3::new(2.5, 2.0, 4.0);
/// Frustum margin on the fitted distance (slack for the spinning model).
const FRAMING_MARGIN: f32 = 1.15;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — materials_pbr_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .init_resource::<CameraFraming>()
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate, report_when_loaded, frame_camera))
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

/// Fit-to-content state machine: armed by the `WorldInstanceReady` observer,
/// polled until `Aabb`s exist (or the timeout keeps the default framing).
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
}

/// Flags the loader gates each PBR scalar on, captured from an `ufbx` probe so the
/// post-load print can say "authored value applied" vs "not authored → Bevy default".
/// Indexed by the dense `Material{N}` order shared with the loader's labels.
#[derive(Resource, Default)]
struct AuthoredMaterials {
    flags: Vec<AuthoredMaterial>,
    names: Vec<String>,
}

#[derive(Default, Clone, Copy)]
struct AuthoredMaterial {
    specular_factor: Option<f64>,
    specular_color: Option<[f64; 3]>,
    specular_anisotropy: Option<f64>,
    specular_rotation: Option<f64>,
    coat_enabled: bool,
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    compressed: Res<FbxCompressedImageFormatSupport>,
) {
    let fixture = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_FIXTURE.to_string());
    assert!(
        Path::new("assets").join(&fixture).is_file(),
        "missing assets/{fixture} — pass a file under assets/ (default: {DEFAULT_FIXTURE})"
    );

    println!("materials_pbr_fbx — PBR scalar parity (specular / anisotropy / clearcoat)");
    println!("fixture: assets/{fixture}");
    println!(
        "FbxCompressedImageFormatSupport = {:?}: compressed container formats (DDS/KTX2 …) \
         the render device can decode; others are skipped by the loader (warn + no image)",
        compressed.0
    );
    let authored = probe_authored(&fixture);
    if authored.flags.is_empty() {
        println!("ufbx probe produced no materials; skipping authored-flag report");
    }
    commands.insert_resource(authored);

    // Load ONLY the Scene0 label: one loader run publishes every labeled sub-asset
    // (StandardMaterial / Image / …) and the WorldAsset holds them alive. The root
    // `Fbx` container is not referenced by this load, so the post-load report reads
    // `Assets<StandardMaterial>` directly, paired with the ufbx probe by dense
    // `Material{N}` index — and there is no second default-settings loader pass.
    commands
        .spawn((
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(fixture))),
            Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
            Spinning,
        ))
        .observe(scene_ready);

    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        // near=0.01 (Bevy default is 0.1) lets the fit-to-content framing get
        // close to tiny CLI fixtures — Bevy's reverse-infinite-Z projection
        // keeps depth precision fine at this near/far ratio. Framing system
        // repositions the camera once content bounds exist; this is the
        // fallback view for the timeout path.
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_xyz(2.5, 2.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Ground,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.25, 0.28, 0.32))),
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

    commands.spawn((
        Text::new(
            "materials_pbr_fbx - PBR scalar parity (specular / anisotropy / clearcoat)\n\
             Look for in stdout: authored flags from the ufbx probe at startup, then the\n\
             resulting StandardMaterial values after load (reflectance = factor x 0.5).\n\
             Coat TEXTURE slots need --features pbr_multi_layer_material_textures\n\
             (compiled out otherwise; startup prints which variant is active).",
        ),
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

/// Parse the fixture with the same `ufbx` options the loader uses and print which PBR
/// scalars the file authors (the loader applies each map only when `has_value`).
fn probe_authored(fixture: &str) -> AuthoredMaterials {
    let path = format!("assets/{fixture}");
    println!("--- authored in file (ufbx probe, loader-matching opts): assets/{fixture} ---");

    let Ok(probe) = ufbx::load_file(
        path.as_str(),
        ufbx::LoadOpts {
            ignore_all_content: true,
            load_external_files: false,
            ..Default::default()
        },
    ) else {
        println!("  ufbx probe failed to open the file; skipping authored flags");
        return AuthoredMaterials::default();
    };
    let is_blender = matches!(
        probe.metadata.exporter,
        ufbx::Exporter::BlenderBinary | ufbx::Exporter::BlenderAscii
    );
    let scene = match ufbx::load_file(
        path.as_str(),
        ufbx::LoadOpts {
            target_unit_meters: 1.0,
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_camera_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_light_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            space_conversion: if is_blender {
                ufbx::SpaceConversion::AdjustTransforms
            } else {
                ufbx::SpaceConversion::ModifyGeometry
            },
            geometry_transform_handling: ufbx::GeometryTransformHandling::HelperNodes,
            inherit_mode_handling: ufbx::InheritModeHandling::Compensate,
            generate_missing_normals: true,
            load_external_files: false,
            use_blender_pbr_material: is_blender,
            ..Default::default()
        },
    ) {
        Ok(scene) => scene,
        Err(e) => {
            println!("  ufbx probe failed: {e:?}");
            return AuthoredMaterials::default();
        }
    };

    let scalar = |label: &str, m: &ufbx::MaterialMap| -> Option<f64> {
        if m.has_value {
            println!("    {label} = {}", m.value_vec4.x);
            Some(m.value_vec4.x)
        } else {
            println!("    {label} = not authored (loader keeps the Bevy default)");
            None
        }
    };

    let mut authored = Vec::new();
    let mut names = Vec::new();
    for mat in scene
        .materials
        .as_ref()
        .iter()
        .filter(|m| m.element.element_id != 0)
    {
        let name = &mat.element.name;
        names.push(name.to_string());
        println!("  material '{name}':");
        let specular_factor = scalar("specular_factor", &mat.pbr.specular_factor);
        if let Some(f) = specular_factor {
            println!("      → loader: reflectance = {f} x 0.5 = {}", f * 0.5);
        }
        let specular_color = if mat.pbr.specular_color.has_value {
            let v = mat.pbr.specular_color.value_vec4;
            println!("    specular_color = ({}, {}, {})", v.x, v.y, v.z);
            println!(
                "      → loader: specular_tint = linear ({}, {}, {})",
                v.x, v.y, v.z
            );
            Some([v.x, v.y, v.z])
        } else {
            println!("    specular_color = not authored (loader keeps white tint)");
            None
        };
        let specular_anisotropy = scalar("specular_anisotropy", &mat.pbr.specular_anisotropy);
        if specular_anisotropy.is_some() {
            println!("      → loader: anisotropy_strength");
        }
        let specular_rotation = scalar("specular_rotation", &mat.pbr.specular_rotation);
        if specular_rotation.is_some() {
            println!("      → loader: anisotropy_rotation");
        }
        let coat_enabled = mat.features.coat.enabled;
        if coat_enabled {
            println!("    coat feature: enabled");
            let _ = scalar("coat_factor", &mat.pbr.coat_factor);
            let _ = scalar("coat_roughness", &mat.pbr.coat_roughness);
            println!("      → loader: clearcoat / clearcoat_perceptual_roughness");
        } else {
            println!(
                "    coat feature: disabled (loader keeps Bevy clearcoat = 0 / \
                 clearcoat_perceptual_roughness = 0)"
            );
        }
        authored.push(AuthoredMaterial {
            specular_factor,
            specular_color,
            specular_anisotropy,
            specular_rotation,
            coat_enabled,
        });
    }

    #[cfg(not(feature = "pbr_multi_layer_material_textures"))]
    println!(
        "  compiled WITHOUT `pbr_multi_layer_material_textures`: coat TEXTURE slots \
         (clearcoat_texture / clearcoat_roughness_texture / clearcoat_normal_texture) are \
         compiled out — run with --features pbr_multi_layer_material_textures to see coat maps"
    );
    #[cfg(feature = "pbr_multi_layer_material_textures")]
    println!(
        "  compiled WITH `pbr_multi_layer_material_textures`: coat texture slots are \
         present on StandardMaterial (printed per material after load)"
    );

    AuthoredMaterials {
        flags: authored,
        names,
    }
}

/// Print the resulting `StandardMaterial` values once the labeled sub-assets landed.
#[allow(clippy::too_many_arguments)]
fn report_when_loaded(
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    authored: Res<AuthoredMaterials>,
    mut reported: Local<bool>,
    mut settle_frames: Local<u32>,
) {
    if *reported {
        return;
    }
    // Collect `Material{N}/Standard` labels: the root `Fbx` / `FbxMaterial`
    // containers are dropped after a label-only load (nothing holds a strong
    // handle to them), while these StandardMaterials stay alive through the
    // Scene0 WorldAsset. The floor material has no asset path; `Material{N}
    // (inverted)` twins are skipped (same scalar values, different cull).
    let mut found: Vec<(usize, AssetId<StandardMaterial>)> = Vec::new();
    for (id, _material) in materials.iter() {
        let Some(path) = asset_server.get_path(id) else {
            continue;
        };
        let Some(label) = path.label() else {
            continue;
        };
        let Some(rest) = label.strip_prefix("Material") else {
            continue;
        };
        let Some(slash) = rest.find('/') else {
            continue;
        };
        let Ok(index) = rest[..slash].parse::<usize>() else {
            continue;
        };
        found.push((index, id));
    }
    if found.is_empty() {
        return;
    }
    found.sort_by_key(|(index, _)| *index);

    // Texture settle pass: the default fixture references external
    // `assets/textures/checkerboard_*.png` images that are NOT in this corpus.
    // bevy_pbr leaves a material unprepared while a bound image is missing, so
    // its mesh never renders — clear the dead slot (the base-color scalar above
    // still demonstrates the parity) and say so on stdout; images that are
    // still loading delay the report (bounded by 600 frames).
    let mut pending = false;
    let mut cleared: Vec<&'static str> = Vec::new();
    for (_, id) in &found {
        let Some(mut material) = materials.get_mut(*id) else {
            continue;
        };
        // `AssetMut` derefs to the material; settle one field per call so the
        // reborrows stay disjoint.
        let slots: [(
            &'static str,
            fn(&mut StandardMaterial) -> &mut Option<Handle<Image>>,
        ); 5] = [
            ("base_color", |m| &mut m.base_color_texture),
            ("emissive", |m| &mut m.emissive_texture),
            ("metallic_roughness", |m| &mut m.metallic_roughness_texture),
            ("occlusion", |m| &mut m.occlusion_texture),
            ("normal", |m| &mut m.normal_map_texture),
        ];
        for (name, pick) in slots {
            if settle_texture(&asset_server, pick(&mut material), &mut pending) {
                cleared.push(name);
            }
        }
        #[cfg(feature = "pbr_multi_layer_material_textures")]
        {
            let coat_slots: [(
                &'static str,
                fn(&mut StandardMaterial) -> &mut Option<Handle<Image>>,
            ); 3] = [
                ("clearcoat", |m| &mut m.clearcoat_texture),
                ("clearcoat_roughness", |m| {
                    &mut m.clearcoat_roughness_texture
                }),
                ("clearcoat_normal", |m| &mut m.clearcoat_normal_texture),
            ];
            for (name, pick) in coat_slots {
                if settle_texture(&asset_server, pick(&mut material), &mut pending) {
                    cleared.push(name);
                }
            }
        }
    }
    if pending {
        *settle_frames += 1;
        if *settle_frames < 600 {
            return;
        }
    }
    if !cleared.is_empty() {
        cleared.sort_unstable();
        cleared.dedup();
        println!(
            "note: external image(s) missing from the corpus (assets/textures/...): cleared \
             [{}] texture slot(s) so the mesh renders — the loader kept the asset-server \
             reference (see the Path not found / WARN lines above)",
            cleared.join(", ")
        );
    }

    println!("--- loaded StandardMaterial (label Material{{N}}/Standard → values) ---");
    for (index, id) in found {
        let Some(material) = materials.get(id) else {
            continue;
        };
        let flags = authored.flags.get(index);
        let name = authored
            .names
            .get(index)
            .map(String::as_str)
            .unwrap_or("<unknown>");

        let factor_note = match flags.and_then(|a| a.specular_factor) {
            Some(v) => format!("authored {v} x 0.5"),
            None if flags.is_some() => {
                "specular_factor not authored → Bevy default kept".to_string()
            }
            None => "probe unavailable".to_string(),
        };
        let tint_note = match flags.and_then(|a| a.specular_color) {
            Some(v) => format!("authored specular_color ({}, {}, {})", v[0], v[1], v[2]),
            None if flags.is_some() => "specular_color not authored → white kept".to_string(),
            None => "probe unavailable".to_string(),
        };
        let aniso_note =
            |label: &str, pick: fn(&AuthoredMaterial) -> Option<f64>| match flags.and_then(pick) {
                Some(v) => format!("authored {label} {v}"),
                None if flags.is_some() => format!("{label} not authored → Bevy default 0"),
                None => "probe unavailable".to_string(),
            };
        let coat_note = match flags {
            Some(a) if a.coat_enabled => {
                "coat feature enabled → applied from coat maps".to_string()
            }
            Some(_) => "coat feature disabled → Bevy defaults (0)".to_string(),
            None => "probe unavailable".to_string(),
        };

        println!("  material '{name}' (Material{index}):");
        println!(
            "    reflectance           = {:.4}  ({factor_note})",
            material.reflectance
        );
        println!(
            "    specular_tint         = {:?}  ({tint_note})",
            material.specular_tint
        );
        println!(
            "    anisotropy_strength   = {:.4}  ({})",
            material.anisotropy_strength,
            aniso_note("specular_anisotropy", |a| a.specular_anisotropy)
        );
        println!(
            "    anisotropy_rotation   = {:.4}  ({})",
            material.anisotropy_rotation,
            aniso_note("specular_rotation", |a| a.specular_rotation)
        );
        println!(
            "    clearcoat             = {:.4}  ({coat_note})",
            material.clearcoat
        );
        println!(
            "    clearcoat_roughness   = {:.4}",
            material.clearcoat_perceptual_roughness
        );
        println!(
            "    metallic/roughness    = {:.4} / {:.4}",
            material.metallic, material.perceptual_roughness
        );
        println!(
            "    base_color_texture    = {}",
            if material.base_color_texture.is_some() {
                "present"
            } else {
                "absent"
            }
        );
        #[cfg(feature = "pbr_multi_layer_material_textures")]
        println!(
            "    clearcoat textures    = factor:{} roughness:{} normal:{} \
             (pbr_multi_layer_material_textures ON)",
            material.clearcoat_texture.is_some(),
            material.clearcoat_roughness_texture.is_some(),
            material.clearcoat_normal_texture.is_some(),
        );
    }
    #[cfg(not(feature = "pbr_multi_layer_material_textures"))]
    println!(
        "note: coat TEXTURE slots (clearcoat_texture / clearcoat_roughness_texture / \
         clearcoat_normal_texture) compile out without bevy's `pbr_multi_layer_material_textures` \
         — run with --features pbr_multi_layer_material_textures to see coat maps"
    );
    *reported = true;
}

/// Scene instance spawned → arm the fit-to-content camera framing.
fn scene_ready(_ready: On<WorldInstanceReady>, mut framing: ResMut<CameraFraming>) {
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
    }
}

/// Fit the camera to the measured content bounds (the `load_fbx` pattern),
/// spin-proofed for the rotating model: the demo spins the fixture about the Y
/// axis through the origin, so the frame is a **spin-invariant** bounding
/// sphere around that axis (horizontal distance + height do not change as the
/// model turns, unlike the raw union `Aabb`). Ground and shadow cascades are
/// re-fitted from the same measurement, so any CLI fixture — metre-cube or
/// centimetre trinket — lands framed and shadowed instead of sitting inside the
/// camera.
#[allow(clippy::too_many_arguments)]
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
    mut cascades: Query<&mut CascadeShadowConfig, With<DirectionalLight>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !framing.armed || framing.done {
        return;
    }
    framing.frames += 1;

    // Union of world-space mesh bounds (ground excluded: it is dressing, not
    // content). `Aabb` is inserted by Bevy's calculate_bounds after spawn.
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut count = 0usize;
    for (aabb, global) in meshes.iter() {
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
        count += 1;
    }
    if count == 0 {
        if framing.frames >= FRAMING_TIMEOUT_FRAMES {
            framing.armed = false;
            framing.done = true;
            println!(
                "[materials_pbr_fbx] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
                 keeping default camera framing"
            );
        }
        return;
    }
    framing.armed = false;
    framing.done = true;

    // Spin-invariant sphere about the Y axis through the origin (the `Spinning`
    // root rotates the model about that axis).
    let center = Vec3::new(0.0, (min.y + max.y) * 0.5, 0.0);
    let mut radius = (max.y - min.y).abs() * 0.5;
    for (aabb, global) in meshes.iter() {
        let (lo, hi) = (aabb.min(), aabb.max());
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for z in [lo.z, hi.z] {
                    let world = global.transform_point(Vec3::new(x, y, z));
                    radius = radius.max(Vec3::new(world.x, world.y, world.z).distance(center));
                }
            }
        }
    }
    radius = radius.max(1e-4);

    {
        let mut ground_q = rigs.p1();
        if let Ok(mut ground) = ground_q.single_mut() {
            ground.translation.y = min.y - 0.06 * radius;
            ground.scale = Vec3::splat((radius * 0.6).max(1.0));
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
                w.resolution.width() as f32 / h as f32
            } else {
                1.0
            }
        })
        .unwrap_or(1.0)
        .max(0.1);
    let h_fov = 2.0 * ((0.5 * v_fov).tan() * aspect).atan();
    let half_angle = 0.5 * v_fov.min(h_fov);
    let mut distance = radius / half_angle.sin() * FRAMING_MARGIN;
    // Keep the model outside the near plane for tiny content.
    distance = distance.max(near * 1.5 + radius);

    let pos = center + VIEW_DIR.normalize() * distance;
    *cam_tf = Transform::from_translation(pos).looking_at(center, Vec3::Y);

    println!("[materials_pbr_fbx] content bounds: {count} mesh AABB(s)");
    println!(
        "[materials_pbr_fbx]   world min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4}) \
         center=({:.4}, {:.4}, {:.4}) radius={:.4} (spin-invariant)",
        min.x, min.y, min.z, max.x, max.y, max.z, center.x, center.y, center.z, radius
    );
    println!(
        "[materials_pbr_fbx] camera framing: distance={distance:.4} v_fov={v_fov:.4}rad \
         aspect={aspect:.3} near={near:.4} translation=({:.4}, {:.4}, {:.4})",
        pos.x, pos.y, pos.z
    );
}

/// Clear a texture slot whose image failed to load (bevy_pbr would keep the
/// material unprepared and never draw its mesh); flag `Loading` so the caller
/// waits for a terminal state instead of reporting half-settled materials.
fn settle_texture(
    asset_server: &AssetServer,
    slot: &mut Option<Handle<Image>>,
    pending: &mut bool,
) -> bool {
    let Some(handle) = slot.as_ref() else {
        return false;
    };
    match asset_server.load_state(handle.id()) {
        LoadState::Failed(_) => {
            *slot = None;
            true
        }
        LoadState::Loading => {
            *pending = true;
            false
        }
        _ => false,
    }
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
