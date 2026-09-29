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
//! the parity is visible in stdout. NOTE: the default scale assumes this Blender cm
//! fixture; a differently-scaled file passed on the CLI may need `DEMO_VISUAL_SCALE`
//! adjusted to frame it.

use std::f32::consts::PI;
use std::path::Path;

use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{FbxAssetLabel, FbxCompressedImageFormatSupport, FbxPlugin};

const DEFAULT_FIXTURE: &str = "blender_279_internal_textures_7400_binary.fbx";
/// Blender 2.79 fixture reports centimetre-ish extents after conversion — enlarge.
const DEMO_VISUAL_SCALE: f32 = 100.0;

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
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate, report_when_loaded))
        .run();
}

#[derive(Component)]
struct Spinning;

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
    commands.spawn((
        WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(fixture))),
        Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
        Spinning,
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(2.5, 2.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
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
            "materials_pbr_fbx — PBR scalar parity (specular / anisotropy / clearcoat)\n\
             Look for: stdout — authored flags from the ufbx probe at startup, then the\n\
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
fn report_when_loaded(
    asset_server: Res<AssetServer>,
    materials: Res<Assets<StandardMaterial>>,
    authored: Res<AuthoredMaterials>,
    mut reported: Local<bool>,
) {
    if *reported {
        return;
    }
    // Collect `Material{N}/Standard` labels: the root `Fbx` / `FbxMaterial`
    // containers are dropped after a label-only load (nothing holds a strong
    // handle to them), while these StandardMaterials stay alive through the
    // Scene0 WorldAsset. The floor material has no asset path; `Material{N}
    // (inverted)` twins are skipped (same scalar values, different cull).
    let mut found: Vec<(usize, &StandardMaterial)> = Vec::new();
    for (id, material) in materials.iter() {
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
        found.push((index, material));
    }
    if found.is_empty() {
        return;
    }
    found.sort_by_key(|(index, _)| *index);

    println!("--- loaded StandardMaterial (label Material{{N}}/Standard → values) ---");
    for (index, material) in found {
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

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
