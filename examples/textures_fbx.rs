//! External FBX textures: routed loading, binding AND visible UV sampling.
//!
//! ```sh
//! cargo run --example textures_fbx
//! ```
//!
//! Two subjects, both referencing external PNGs (nothing is embedded):
//!
//! * LEFT — `assets/blender_279_internal_textures_7400_binary.fbx`: the
//!   router story. Its `*.fbx` resolves from `assets/`; its
//!   `textures/checkerboard_*.png` are NOT vendored into `assets/` (the ufbx
//!   corpus ships them at `libs/ufbx/data/textures/`). The mesh genuinely has
//!   no UV set — ufbx reports none, confirmed independently (faithful, not a
//!   loader bug) — so the bound checkerboard cannot be sampled and this cube
//!   renders WHITE. Loading works; sampling is impossible for this fixture.
//! * RIGHT — `blender_293_textures_7400_binary.fbx` (corpus-only): the same
//!   external `textures/checkerboard_*.png` references PLUS a real UV set
//!   (`LayerElementUV`, `UVMap`, ByPolygonVertex), so its base-color actually
//!   samples: the red/dark-red checkerboard is visible on screen.
//!
//! To make any of this work the default asset source is swapped for a router
//! before `DefaultPlugins`: everything (including `*.fbx`) reads `assets/`
//! first (so user copies win), falling back to the ufbx corpus — that is how
//! both the PNGs and the corpus-only UV fixture arrive. Startup stdout states
//! which source serves each fixture and the PNGs, and warns loudly when the
//! PNGs are in neither location (then the subjects stay untextured and the
//! loader logs its own "External texture could not be read" warning). After
//! both scenes settle, stdout reports each `textures/*.png` load state plus
//! how many fixture `StandardMaterial`s hold a `base_color_texture`. The
//! `[diag]` lines carry the proof: `uv0=` range per mesh (MISSING vs a range
//! inside 0..1) and `tex_img=WxH ... px00/pxMid` of the bound image bytes.

use std::f32::consts::PI;
use std::path::Path;

use bevy::asset::LoadState;
use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, PathStream, Reader,
    file::FileAssetReader,
};
use bevy::world_serialization::{WorldAsset, WorldInstanceReady};
use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

/// LEFT subject, the original router story: external PNG refs, mesh has NO UV
/// set (ufbx reports none — faithful) so its texture binds but cannot sample.
/// Served from `assets/`.
const FIXTURE_NO_UV: &str = "blender_279_internal_textures_7400_binary.fbx";
/// RIGHT subject: UV-bearing corpus-only fixture with the same external
/// `textures/checkerboard_*.png` references — its checkerboard actually
/// samples. Exercises the router's corpus fallback for `*.fbx`.
const FIXTURE_UV: &str = "blender_293_textures_7400_binary.fbx";
/// Subjects' authored size after conversion: `dump_fbx` reports the left cube
/// `Aabb` `min=(-1,-1,-1)` `max=(1,1,1)` (metre-scale, unit_scale=1) — kept at
/// 1.0. It used to be 100.0 ("centimetre-ish extents after conversion"), which
/// made the cube 200 units wide: the fixed camera then sat INSIDE it,
/// back-face culling hid every face and the window showed only the ground plane.
const DEMO_VISUAL_SCALE: f32 = 1.0;
/// Subjects sit side by side (LEFT at -X, RIGHT at +X).
const SUBJECT_X: f32 = 1.9;

/// Both fixtures' external texture references (asset-root-relative,
/// `/`-normalized by the loader from Blender's `textures\checkerboard_*.png`
/// RelativeFilename): the no-UV fixture refs diffuse/ambient/emissive/specular/
/// weight, the UV fixture refs diffuse/emissive/metallic/roughness/weight.
const TEXTURE_FILES: [&str; 7] = [
    "textures/checkerboard_diffuse.png",
    "textures/checkerboard_ambient.png",
    "textures/checkerboard_emissive.png",
    "textures/checkerboard_specular.png",
    "textures/checkerboard_weight.png",
    "textures/checkerboard_metallic.png",
    "textures/checkerboard_roughness.png",
];

/// ufbx corpus root (the checkerboard PNGs). Optional at runtime.
const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../libs/ufbx/data");

fn main() {
    let mut app = App::new();

    // External texture bytes (and corpus-only fixtures) are read through the
    // DEFAULT asset source with plain paths, so it must route: `assets/` first
    // (user copies win — this is how `*.fbx` resolves to assets/), then the
    // ufbx corpus (the fixture PNGs and the UV-bearing subject's FBX). Must run
    // BEFORE DefaultPlugins builds the AssetServer.
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
            title: "bevy_ufbx — textures_fbx".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(FbxPlugin)
    .add_systems(Startup, setup)
    .add_systems(Update, (rotate, report_when_loaded))
    .run();
}

// ----------------------------------------------------------------------------
// Routing asset source
// ----------------------------------------------------------------------------

/// Serves every file from `assets/` first with a fallback to the ufbx corpus.
///
/// Covers the fixture's external PNGs (not vendored into `assets/`) and the
/// corpus-only UV-bearing subject (`*.fbx` also falls back, while copies in
/// `assets/` keep precedence).
///
/// Replaces the default source's reader because the loader's texture pre-reads
/// and fallback loads always go through the default source with plain paths.
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

impl AssetReader for RoutingReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // `assets/` first (user copies in assets/ win), corpus fallback.
        // Inlined (not a helper): every branch must be the SAME concrete reader
        // type to satisfy the single opaque return type.
        match self.assets.read(path).await {
            Ok(reader) => Ok(reader),
            Err(AssetReaderError::NotFound(_)) => self.corpus.read(path).await,
            Err(err) => Err(err),
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        match self.assets.read_meta(path).await {
            Ok(reader) => Ok(reader),
            Err(AssetReaderError::NotFound(_)) => self.corpus.read_meta(path).await,
            Err(err) => Err(err),
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
// Demo
// ----------------------------------------------------------------------------

#[derive(Component)]
struct Spinning;

/// Scene handles for both subjects + probe handles for the fixtures' external
/// PNGs (requested so the report can watch their load states; the loader
/// requests the same paths).
#[derive(Resource)]
struct TexturesFixture {
    scenes: Vec<Handle<WorldAsset>>,
    probes: Vec<Handle<Image>>,
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut scenes = Vec::new();
    for (fixture, x) in [(FIXTURE_NO_UV, -SUBJECT_X), (FIXTURE_UV, SUBJECT_X)] {
        let in_assets = Path::new("assets").join(fixture).is_file();
        let in_corpus = Path::new(CORPUS).join(fixture).is_file();
        assert!(
            in_assets || in_corpus,
            "missing {fixture} — looked in assets/ and {CORPUS} (copy from ufbx/data)"
        );
        let source = if in_assets {
            "assets/"
        } else {
            "ufbx corpus (router fallback)"
        };
        println!("fixture {fixture}: served from {source}");

        let scene = asset_server.load(FbxAssetLabel::Scene(0).from_asset(fixture));
        commands
            .spawn((
                WorldAssetRoot(scene.clone()),
                Transform::from_xyz(x, 0.0, 0.0).with_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
                Spinning,
            ))
            .observe(diagnose_spawn);
        scenes.push(scene);
    }

    println!("textures_fbx — external texture loading (fixture references, not embeds)");
    let in_assets = TEXTURE_FILES
        .iter()
        .all(|file| Path::new("assets").join(file).is_file());
    let in_corpus = TEXTURE_FILES
        .iter()
        .all(|file| Path::new(CORPUS).join(file).is_file());
    if in_assets {
        println!("texture bytes: assets/textures/checkerboard_*.png present — served from assets/");
    } else if in_corpus {
        println!(
            "texture bytes: assets/textures/ absent — falling back to the ufbx corpus at \
             {CORPUS} (checkerboard PNGs come from there)"
        );
    } else {
        eprintln!(
            "WARNING: checkerboard PNGs found neither in assets/textures/ nor {CORPUS}.\n\
             Both subjects will render UNTEXTURED and the loader will warn that external\n\
             textures could not be read. To fix, copy them from libs/ufbx/data/textures/:\n\
             mkdir -p assets/textures && cp ../../libs/ufbx/data/textures/checkerboard_*.png \
             assets/textures/"
        );
    }

    let probes = TEXTURE_FILES
        .iter()
        .map(|file| asset_server.load::<Image>(*file))
        .collect();
    commands.insert_resource(TexturesFixture { scenes, probes });

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.6, 2.4, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
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
            // ASCII only: Bevy's default font (FiraMono-subset) has no "—" glyph.
            "textures_fbx - external PNGs through the routed default asset source.\n\
             LEFT : blender_279_internal_textures (assets/) - PNG loads+binds, but its\n\
             mesh has NO UVs (ufbx reports none) -> cannot sample, cube stays WHITE.\n\
             RIGHT: blender_293_textures (ufbx corpus) - real UV set -> the red\n\
             checkerboard base-color is actually SAMPLED ([diag] uv0= / tex_img=).\n\
             Startup stdout: which source serves textures/checkerboard_*.png;\n\
             after load: each PNG LoadState + bound base_color_textures count.",
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

/// One-shot report when Scene0 lands: world placement, geometry sanity (signed
/// volume + normal direction) and material state of every spawned descendant,
/// so an invisible subject can be explained from stdout alone. Read-only — it
/// never mutates the material (the winding/cull theory it was written for was
/// refuted: `signed_vol` positive means outward winding, `cull=Back` correct).
#[allow(clippy::too_many_arguments)]
fn diagnose_spawn(
    ready: On<WorldInstanceReady>,
    children: Query<&Children>,
    names: Query<&Name>,
    meshes: Query<&Mesh3d>,
    mesh_assets: Res<Assets<Mesh>>,
    globals: Query<&GlobalTransform>,
    vis: Query<&Visibility>,
    mats: Query<&MeshMaterial3d<StandardMaterial>>,
    materials: Res<Assets<StandardMaterial>>,
    images: Res<Assets<Image>>,
) {
    use bevy::render::mesh::{Indices, VertexAttributeValues};

    for child in children.iter_descendants(ready.entity) {
        let name = names
            .get(child)
            .map(|n| n.to_string())
            .unwrap_or_else(|_| "(unnamed)".to_string());
        let gt = globals.get(child).map(|g| g.compute_transform()).ok();
        let tf = match gt {
            Some(t) => format!(
                "t=({:.4},{:.4},{:.4}) s=({:.4},{:.4},{:.4})",
                t.translation.x, t.translation.y, t.translation.z, t.scale.x, t.scale.y, t.scale.z
            ),
            None => "(no GlobalTransform)".to_string(),
        };
        let mesh = if meshes.get(child).is_ok() {
            "MESH"
        } else {
            "-"
        };
        let visibility = vis.get(child).map(|v| format!("{v:?}")).unwrap_or_default();

        let mut geo = String::new();
        if let Ok(m3d) = meshes.get(child)
            && let Some(m) = mesh_assets.get(&m3d.0)
        {
            let n_verts = m
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .map(|a| match a {
                    VertexAttributeValues::Float32x3(v) => v.len(),
                    _ => 0,
                })
                .unwrap_or(0);
            // Signed volume (closed, outward winding => positive) + how far the
            // normals point away from the origin.
            let mut vol = 0.0f64;
            let mut dot = 0.0f64;
            if let (
                Some(VertexAttributeValues::Float32x3(pos)),
                Some(VertexAttributeValues::Float32x3(nrm)),
            ) = (
                m.attribute(Mesh::ATTRIBUTE_POSITION),
                m.attribute(Mesh::ATTRIBUTE_NORMAL),
            ) {
                for (p, n) in pos.iter().zip(nrm.iter()) {
                    dot += (p[0] * n[0] + p[1] * n[1] + p[2] * n[2]) as f64;
                }
                if let Some(Indices::U32(idx)) = m.indices() {
                    for tri in idx.chunks_exact(3) {
                        let (a, b, c) = (
                            pos[tri[0] as usize],
                            pos[tri[1] as usize],
                            pos[tri[2] as usize],
                        );
                        vol += ((a[0] * (b[1] * c[2] - b[2] * c[1])
                            - a[1] * (b[0] * c[2] - b[2] * c[0])
                            + a[2] * (b[0] * c[1] - b[1] * c[0]))
                            as f64)
                            / 6.0;
                    }
                }
            }
            // UVs drive the base-color lookup — missing/degenerate UVs leave
            // the material flat white even with a texture bound.
            let uv = match m.attribute(Mesh::ATTRIBUTE_UV_0) {
                Some(VertexAttributeValues::Float32x2(v)) => {
                    let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                    for t in v {
                        u0 = u0.min(t[0]);
                        u1 = u1.max(t[0]);
                        v0 = v0.min(t[1]);
                        v1 = v1.max(t[1]);
                    }
                    // Per-triangle UV spans + distinct checker tiles (period
                    // 16/128 = 0.125 UV): if every tri stays inside one tile
                    // and each face lands on a single tile, the rendered face
                    // is one flat tile colour even though sampling works.
                    let mut detail = String::new();
                    if let Some(Indices::U32(idx)) = m.indices() {
                        let mut su = (f32::MAX, 0.0f32);
                        let mut sv = (f32::MAX, 0.0f32);
                        for tri in idx.chunks_exact(3) {
                            let us = [
                                v[tri[0] as usize][0],
                                v[tri[1] as usize][0],
                                v[tri[2] as usize][0],
                            ];
                            let vs = [
                                v[tri[0] as usize][1],
                                v[tri[1] as usize][1],
                                v[tri[2] as usize][1],
                            ];
                            let du = us.iter().cloned().fold(f32::MIN, f32::max)
                                - us.iter().cloned().fold(f32::MAX, f32::min);
                            let dv = vs.iter().cloned().fold(f32::MIN, f32::max)
                                - vs.iter().cloned().fold(f32::MAX, f32::min);
                            su.0 = su.0.min(du);
                            su.1 = su.1.max(du);
                            sv.0 = sv.0.min(dv);
                            sv.1 = sv.1.max(dv);
                        }
                        let mut tiles = std::collections::BTreeSet::new();
                        for t in v {
                            tiles
                                .insert(((t[0] * 8.0).floor() as i32, (t[1] * 8.0).floor() as i32));
                        }
                        detail = format!(
                            " uvTriSpan u={:.3}..{:.3} v={:.3}..{:.3} tiles16px={:?}",
                            su.0, su.1, sv.0, sv.1, tiles
                        );
                    }
                    format!(
                        " uv0=({u0:.3}..{u1:.3},{v0:.3}..{v1:.3})[{}]{detail}",
                        v.len()
                    )
                }
                Some(_) => " uv0=wrong-type".to_string(),
                None => " uv0=MISSING".to_string(),
            };
            geo = format!(" verts={n_verts} signed_vol={vol:.6} sum_dot(normal,pos)={dot:.4}{uv}");
        }

        let material = match mats.get(child) {
            Ok(mm) => {
                let before = materials
                    .get(&mm.0)
                    .map(|m| {
                        format!(
                            "mode={:?} cull={:?} tex={} base=({:.2},{:.2},{:.2},{:.2})",
                            m.alpha_mode,
                            m.cull_mode,
                            m.base_color_texture.is_some(),
                            m.base_color.to_srgba().red,
                            m.base_color.to_srgba().green,
                            m.base_color.to_srgba().blue,
                            m.base_color.to_srgba().alpha,
                        )
                    })
                    .unwrap_or_else(|| "(material asset missing)".to_string());
                before
            }
            Err(_) => "-".to_string(),
        };
        // CPU-side probe of the bound base-color image: distinguishes "wrong
        // bytes landed in the asset" from "texture bound but not sampled".
        let texinfo = match mats.get(child) {
            Ok(mm) => match materials
                .get(&mm.0)
                .and_then(|m| m.base_color_texture.as_ref())
            {
                None => String::new(),
                Some(h) => match images.get(h) {
                    None => " tex_img=MISSING".to_string(),
                    Some(img) => {
                        let (w, h2) = (img.size().x, img.size().y);
                        match img.data.as_deref() {
                            None => format!(
                                " tex_img={w}x{h2} data=absent fmt={:?}",
                                img.texture_descriptor.format
                            ),
                            Some(d) => {
                                let px_per = (w * h2) as usize;
                                let bpp = if px_per > 0 { d.len() / px_per } else { 0 };
                                let px = |i: usize| -> String {
                                    let o = i * bpp;
                                    if bpp >= 3 && o + 2 < d.len() {
                                        format!("{}.{}.{}", d[o], d[o + 1], d[o + 2])
                                    } else {
                                        "-".to_string()
                                    }
                                };
                                format!(
                                    " tex_img={w}x{h2} bpp={bpp} fmt={:?} px00={} pxMid={}",
                                    img.texture_descriptor.format,
                                    px(0),
                                    px(((h2 / 2) * w + w / 2) as usize)
                                )
                            }
                        }
                    }
                },
            },
            Err(_) => String::new(),
        };
        println!("[diag] {name} {mesh} {tf} vis={visibility} mat: {material}{geo}{texinfo}");
    }
}

/// One-shot stdout report once both subjects' scenes and the probe PNG loads settle.
fn report_when_loaded(
    asset_server: Res<AssetServer>,
    fixture: Res<TexturesFixture>,
    materials: Res<Assets<StandardMaterial>>,
    mut reported: Local<bool>,
) {
    if *reported {
        return;
    }
    for scene in &fixture.scenes {
        match asset_server.load_state(scene) {
            LoadState::Loaded => {}
            LoadState::Failed(err) => {
                error!("FBX load failed: {err:?}");
                *reported = true;
                return;
            }
            _ => return,
        }
    }
    // Probe requests are registered by the loader run (and mirrored here), so
    // they settle to Loaded/Failed within a frame or two of Scene0 landing.
    if fixture.probes.iter().any(|handle| {
        !matches!(
            asset_server.load_state(handle),
            LoadState::Loaded | LoadState::Failed(_)
        )
    }) {
        return;
    }

    println!("--- external texture bytes (textures/*.png requested by the loader run) ---");
    let mut landed = 0usize;
    for handle in &fixture.probes {
        let state = asset_server.load_state(handle);
        let path = asset_server
            .get_path(handle.id())
            .map(|p| p.path().display().to_string())
            .unwrap_or_default();
        if matches!(state, LoadState::Loaded) {
            landed += 1;
        }
        println!("  {path}: {state:?}");
    }

    // Fixture StandardMaterials (label Material{N}/Standard) that hold a
    // base-color handle. The handle is bound even when the PNG is missing —
    // whether pixels exist is exactly what the states above report.
    let mut total = 0usize;
    let mut bound = 0usize;
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
        if !rest.contains('/') {
            continue; // skip FbxMaterial containers / inverted twins naming
        }
        total += 1;
        if material.base_color_texture.is_some() {
            bound += 1;
        }
    }
    println!(
        "  landed {landed}/{} PNG(s); {bound}/{total} fixture StandardMaterial(s) bound \
              to base_color_texture",
        fixture.probes.len(),
    );
    if landed == 0 {
        println!(
            "  → no external pixels: both subjects render UNTEXTURED (see startup WARNING for \
             the fix)"
        );
    } else {
        println!(
            "  → external pixels served through the routed default source; [diag] uv0= shows \
             which subject can SAMPLE them (MISSING = bound but flat)"
        );
    }
    *reported = true;
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
