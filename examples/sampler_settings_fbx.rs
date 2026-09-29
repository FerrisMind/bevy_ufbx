//! Sampler / settings precedence, four instances of one FBX side by side.
//!
//! ```sh
//! cargo run --example sampler_settings_fbx
//! ```
//!
//! Demonstrates the full precedence chain from `DefaultFbxImageSampler`'s docs
//! (strongest first, each tier wins over everything below it):
//!
//! 1. `FbxLoaderSettings::override_sampler` — used as-is for every texture;
//!    FBX wrap modes are ignored.
//! 2. `FbxLoaderSettings::default_sampler` — per-load base; FBX `wrap_u`/`wrap_v`
//!    overwrite the address modes on top of it. The loader detects "explicitly
//!    set" by `!= ImageSamplerDescriptor::default()` (the serde default is the
//!    "unset" sentinel), so even a `label` makes it count.
//! 3. `DefaultFbxImageSampler` app resource — base for loads that left the
//!    per-load field at its default value.
//! 4. Loader built-in default — `ImageSamplerDescriptor::linear()` (glTF
//!    parity, `GltfPlugin::default()` uses it too) when the resource itself
//!    is untouched.
//!
//! ## Why four asset sources (the honest part)
//!
//! bevy_asset keys a load by the full `AssetPath` — `(source, path, label)` —
//! and returns the *first* load's result for every later request of the same
//! key: settings are applied per loader run, so "spawn the same path twice
//! with different settings" silently keeps only the first settings. To get
//! four independent loader runs over the same bytes, this example loads the
//! fixture through four named asset sources (`s_builtin`, `s_resource`,
//! `s_perload`, `s_override`). Distinct sources = distinct keys = four runs.
//! The `DefaultFbxImageSampler` resource is global, so the run order is
//! phase-gated: `s_builtin` loads first while the resource is untouched
//! (tier 4); only after it settles is the resource set to Nearest, then
//! `s_resource` / `s_perload` / `s_override` load (tiers 3 / 2 / 1).
//!
//! The fixture references `textures/checkerboard_diffuse.png` relative to the
//! asset root, which is not vendored into `assets/`. External texture bytes
//! are always read through the **default** source with plain paths, so the
//! default source's reader is swapped for a router: `*.fbx` → `assets/`,
//! anything else → the ufbx corpus at `libs/ufbx/data` (optional — without it
//! the example still runs and says so; textures just stay untextured).
//! No binary fixtures are added.
//!
//! Look for: four checkerboard cubes whose filtering differs per column
//! (Linear / Nearest / mag-Linear / Linear+Clamp), and stdout printing the
//! requested tier per instance plus the resolved `ImageSamplerDescriptor`
//! per source after all loads settle.

use std::collections::BTreeSet;
use std::f32::consts::PI;
use std::path::Path;

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, PathStream, Reader,
    file::FileAssetReader,
};
use bevy::asset::{AssetApp, LoadState};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::world_serialization::WorldAsset;
use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{DefaultFbxImageSampler, FbxAssetLabel, FbxLoaderSettings, FbxPlugin};

const FIXTURE: &str = "blender_279_internal_textures_7400_binary.fbx";
/// Blender 2.79 fixture reports centimetre-ish extents after conversion — enlarge.
const DEMO_VISUAL_SCALE: f32 = 100.0;

/// Named sources: one per precedence tier, all routing to the same files.
const SRC_BUILTIN: &str = "s_builtin";
const SRC_RESOURCE: &str = "s_resource";
const SRC_PERLOAD: &str = "s_perload";
const SRC_OVERRIDE: &str = "s_override";

/// ufbx corpus root (fixture's external PNGs). Optional at runtime.
const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../libs/ufbx/data");

/// World-space columns, left → right: tiers 4, 3, 2, 1.
const COLUMN_X: [f32; 4] = [-4.5, -1.5, 1.5, 4.5];

fn main() {
    let mut app = App::new();

    // Distinct asset sources = distinct AssetPath keys = independent loader
    // runs over the same FBX bytes (see header). Must run BEFORE DefaultPlugins
    // builds the AssetServer.
    for id in [SRC_BUILTIN, SRC_RESOURCE, SRC_PERLOAD, SRC_OVERRIDE] {
        app.register_asset_source(
            id,
            AssetSourceBuilder::new(|| Box::new(RoutingReader::new())),
        );
    }
    // External texture reads/fallbacks always use the DEFAULT source with plain
    // paths, so it must route too: `*.fbx` → assets/, everything else → corpus.
    app.register_asset_source(
        AssetSourceId::Default,
        AssetSourceBuilder::new(|| Box::new(RoutingReader::new())),
    );

    app.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 2000.,
        ..default()
    })
    .add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "bevy_ufbx — sampler_settings_fbx".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(FbxPlugin)
    .add_systems(Startup, setup)
    .add_systems(Update, (rotate, advance))
    .run();
}

// ----------------------------------------------------------------------------
// Routing asset source
// ----------------------------------------------------------------------------

/// Serves `*.fbx` from `assets/` and everything else from the ufbx corpus.
///
/// Replaces the reader of every source used here (named ones and the default),
/// because the loader's texture pre-reads and fallback loads always go through
/// the default source with plain paths.
struct RoutingReader {
    assets: FileAssetReader,
    corpus: FileAssetReader,
}

impl RoutingReader {
    fn new() -> Self {
        Self {
            assets: FileAssetReader::new("assets"),
            // Absolute path replaces the base path inside FileAssetReader::new.
            corpus: FileAssetReader::new(CORPUS),
        }
    }
}

fn is_fbx(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("fbx"))
}

impl AssetReader for RoutingReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if is_fbx(path) {
            self.assets.read(path).await
        } else {
            self.corpus.read(path).await
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if is_fbx(path) {
            self.assets.read_meta(path).await
        } else {
            self.corpus.read_meta(path).await
        }
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        self.assets.read_directory(path).await
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        self.assets.is_directory(path).await
    }
}

// ----------------------------------------------------------------------------
// Precedence demo
// ----------------------------------------------------------------------------

#[derive(Component)]
struct Spinning;

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    /// Tier 4: load s_builtin with the resource untouched.
    Builtin,
    /// Tiers 3/2/1: resource set to Nearest, then s_resource/s_perload/s_override.
    Rest,
    /// All settled: print resolved samplers per source.
    Report,
    Done,
}

#[derive(Resource)]
struct Demo {
    phase: Phase,
    builtin: Handle<WorldAsset>,
    rest: Vec<Handle<WorldAsset>>,
}

/// Tier 2 descriptor: differs from `ImageSamplerDescriptor::default()` (label +
/// mag filter), which is how the loader detects an explicitly-set per-load
/// value. Mag Linear / min Nearest makes the column visibly between tiers 4/3.
fn per_load_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        label: Some("per-load: FbxLoaderSettings::default_sampler".to_string()),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Nearest,
        ..ImageSamplerDescriptor::default()
    }
}

/// Tier 1 descriptor: used as-is — Linear filters, ClampToEdge address modes,
/// so the FBX Repeat wrap does NOT apply (the visual tell for the top tier).
fn override_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        label: Some("override: FbxLoaderSettings::override_sampler".to_string()),
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    }
}

/// Tier 3 descriptor stored in the `DefaultFbxImageSampler` resource: an
/// explicit Nearest build (the built-in default is now `linear()`), so the
/// blocky column stays visibly distinct from tier 4.
fn resource_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        label: Some("app-resource: DefaultFbxImageSampler".to_string()),
        ..ImageSamplerDescriptor::nearest()
    }
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    sampler: Res<DefaultFbxImageSampler>,
) {
    assert!(
        Path::new("assets").join(FIXTURE).is_file(),
        "missing assets/{FIXTURE} — copy from ufbx/data"
    );

    println!("sampler_settings_fbx — precedence chain, four instances of one FBX");
    println!("fixture: assets/{FIXTURE}");
    let corpus_ok = Path::new(CORPUS)
        .join("textures/checkerboard_diffuse.png")
        .is_file();
    if corpus_ok {
        println!("texture bytes: default asset source routed to the ufbx corpus ({CORPUS})");
    } else {
        println!(
            "texture bytes: ufbx corpus NOT found at {CORPUS} — external textures cannot \
             be served; cubes render untextured and the sampler report only covers images \
             that exist"
        );
    }
    print_fixture_wrap();

    println!("caching: bevy_asset keys loads by (source, path, label) and the FIRST load's");
    println!("         settings win for every later request of that key, so four named");
    println!("         asset sources give four independent loader runs over the same file:");
    println!(
        "  [4] {SRC_BUILTIN}  no settings, resource untouched   → built-in default (Linear, glTF parity)"
    );
    println!("  [3] {SRC_RESOURCE} no settings, resource = Nearest  → app resource tier");
    println!("  [2] {SRC_PERLOAD}  default_sampler ≠ default()     → per-load tier");
    println!("  [1] {SRC_OVERRIDE} override_sampler as-is          → override tier (ignores wrap)");
    println!(
        "phase 1: requesting [4] {SRC_BUILTIN} while the resource is untouched = {:?}",
        sampler.get()
    );

    // Tier 4 first, resource untouched.
    let builtin = asset_server.load::<WorldAsset>(
        FbxAssetLabel::Scene(0).from_asset(format!("{SRC_BUILTIN}://{FIXTURE}")),
    );
    spawn_instance(&mut commands, builtin.clone(), COLUMN_X[0]);

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 2.8, 10.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));

    // Overlay camera for Text2d plates (does not clear the 3D color buffer).
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(30.0, 14.0))),
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
            first_cascade_far_bound: 30.0,
            maximum_distance: 60.0,
            ..default()
        }
        .build(),
    ));

    // Name plates (Text2d under Camera2d), matching COLUMN_X order L→R.
    let plates = [
        (-460.0, "4 · built-in\n(no settings: Linear + Repeat)"),
        (-150.0, "3 · resource\n(DefaultFbxImageSampler = Nearest)"),
        (160.0, "2 · per-load\n(default_sampler set)"),
        (470.0, "1 · override\n(override_sampler: Linear + Clamp)"),
    ];
    for (x, label) in plates {
        commands.spawn((
            Text2d::new(label),
            TextFont::from_font_size(24.0),
            TextColor(Color::srgb(1.0, 0.95, 0.7)),
            TextLayout::justify(Justify::Center),
            Transform::from_xyz(x, 270.0, 0.0),
        ));
    }

    commands.spawn((
        Text::new(
            "sampler_settings_fbx — same FBX, four sampler-precedence tiers\n\
             LEFT→RIGHT: [4] built-in | [3] resource | [2] per-load | [1] override\n\
             bevy_asset caches by (source, path, label): same path would keep only the\n\
             first settings, so each tier loads through its own named asset source.\n\
             Look for: checkerboard filtering differs per column; stdout prints the\n\
             resolved ImageSamplerDescriptor per source after all loads settle.",
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

    commands.insert_resource(Demo {
        phase: Phase::Builtin,
        builtin,
        rest: Vec::new(),
    });
}

fn spawn_instance(commands: &mut Commands, handle: Handle<WorldAsset>, x: f32) {
    commands.spawn((
        WorldAssetRoot(handle),
        Transform::from_xyz(x, 0.0, 0.0).with_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
        Spinning,
    ));
}

fn advance(
    mut demo: ResMut<Demo>,
    asset_server: Res<AssetServer>,
    sampler: Res<DefaultFbxImageSampler>,
    images: Res<Assets<Image>>,
    mut commands: Commands,
) {
    match demo.phase {
        Phase::Builtin => {
            if !settled(&asset_server, &demo.builtin) {
                return;
            }
            println!(
                "[4] {SRC_BUILTIN} settled; resource at its load was untouched = {:?}",
                sampler.get()
            );
            println!("    → built-in tier: Linear filters (glTF parity), FBX wrap applied on top");

            // Tier 3/2/1 only: mutate the global resource AFTER tier 4 settled.
            sampler.set(&resource_sampler());
            println!(
                "phase 2: DefaultFbxImageSampler.set({:?}) — applies to loads requested from now on",
                resource_sampler()
            );

            let resource_handle = asset_server.load::<WorldAsset>(
                FbxAssetLabel::Scene(0).from_asset(format!("{SRC_RESOURCE}://{FIXTURE}")),
            );
            println!(
                "[3] {SRC_RESOURCE}: no per-load settings → resolve_default_sampler picks the \
                 resource ({:?}) over the built-in default",
                resource_sampler()
            );
            spawn_instance(&mut commands, resource_handle.clone(), COLUMN_X[1]);

            let per_load_handle = asset_server
                .load_builder()
                .with_settings(|s: &mut FbxLoaderSettings| {
                    s.default_sampler = per_load_sampler();
                })
                .load::<WorldAsset>(
                    FbxAssetLabel::Scene(0).from_asset(format!("{SRC_PERLOAD}://{FIXTURE}")),
                );
            println!(
                "[2] {SRC_PERLOAD}: default_sampler = {:?} — != ImageSamplerDescriptor::default() \
                 so the loader treats it as set and it beats the Nearest resource",
                per_load_sampler()
            );
            spawn_instance(&mut commands, per_load_handle.clone(), COLUMN_X[2]);

            let override_handle = asset_server
                .load_builder()
                .with_settings(|s: &mut FbxLoaderSettings| {
                    s.override_sampler = Some(override_sampler());
                })
                .load::<WorldAsset>(
                    FbxAssetLabel::Scene(0).from_asset(format!("{SRC_OVERRIDE}://{FIXTURE}")),
                );
            println!(
                "[1] {SRC_OVERRIDE}: override_sampler = {:?} — used as-is (beats per-load, \
                 resource, built-in, and the FBX Repeat wrap)",
                override_sampler()
            );
            spawn_instance(&mut commands, override_handle.clone(), COLUMN_X[3]);

            demo.rest = vec![resource_handle, per_load_handle, override_handle];
            demo.phase = Phase::Rest;
        }
        Phase::Rest => {
            if demo
                .rest
                .iter()
                .all(|handle| settled(&asset_server, handle))
            {
                demo.phase = Phase::Report;
            }
        }
        Phase::Report => {
            report(&asset_server, &images);
            demo.phase = Phase::Done;
        }
        Phase::Done => {}
    }
}

/// LoadState::Loaded as soon as the loader run lands its labeled assets —
/// deliberately NOT `is_loaded_with_dependencies`, which would hang the demo
/// forever when an external texture image fails to load (corpus absent).
fn settled(asset_server: &AssetServer, handle: &Handle<WorldAsset>) -> bool {
    match asset_server.load_state(handle.id()) {
        LoadState::Loaded => true,
        LoadState::Failed(err) => {
            error!("FBX load failed: {err:?}");
            true
        }
        _ => false,
    }
}

/// Ground truth: every `Image` produced by each tier's loader run.
fn report(asset_server: &AssetServer, images: &Assets<Image>) {
    println!("--- resolved image samplers per instance (ground truth: Assets<Image>) ---");
    for (rank, source) in [
        (4, SRC_BUILTIN),
        (3, SRC_RESOURCE),
        (2, SRC_PERLOAD),
        (1, SRC_OVERRIDE),
    ] {
        let mut descriptors = BTreeSet::new();
        let mut count = 0usize;
        for (id, image) in images.iter() {
            let Some(path) = asset_server.get_path(id) else {
                continue;
            };
            if path.source().as_str() != Some(source) {
                continue;
            }
            count += 1;
            descriptors.insert(match &image.sampler {
                ImageSampler::Descriptor(descriptor) => format!("{descriptor:?}"),
                ImageSampler::Default => "ImageSampler::Default (global ImagePlugin)".to_string(),
            });
        }
        if count == 0 {
            println!("[{rank}] {source}: no images (external texture unavailable?)");
            continue;
        }
        println!("[{rank}] {source}: {count} image(s)");
        for descriptor in &descriptors {
            println!("      {descriptor}");
        }
    }
    println!(
        "chain: override_sampler > per-load default_sampler (!= default()) > \
         DefaultFbxImageSampler resource > built-in default; FBX wrap_u/v overwrite \
         address_mode_u/v unless override_sampler is set"
    );
}

/// The fixture's authored wrap modes explain where the address modes come from.
fn print_fixture_wrap() {
    let path = format!("assets/{FIXTURE}");
    let Ok(scene) = ufbx::load_file(
        path.as_str(),
        ufbx::LoadOpts {
            load_external_files: false,
            ..Default::default()
        },
    ) else {
        return;
    };
    for texture in scene.textures.as_ref().iter() {
        if texture.content.is_empty() && !texture.filename.is_empty() {
            println!(
                "fixture texture '{}' wrap_u={:?} wrap_v={:?} (external ref → address modes \
                 come from here for tiers 4-2; tier 1 override ignores them)",
                texture.filename, texture.wrap_u, texture.wrap_v
            );
        }
    }
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
