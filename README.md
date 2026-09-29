# bevy_ufbx

FBX asset loader for [Bevy](https://bevyengine.org) powered by [ufbx](https://github.com/ufbx/ufbx).

Targets **Bevy glTF-class runtime parity** for FBX: scenes, skins, baked animation (TRS + morph weights), materials/textures, lights/cameras, NURBS tessellation — everything Bevy can consume. **Not** a 1:1 Autodesk FBX runtime and **not** a complete Autodesk solver: no live IK/constraint/dual-quaternion solvers — bake those in your DCC or rely on `ufbx::bake_anim` TRS output.

## Bevy compatibility

| bevy | bevy_ufbx |
|------|-----------|
| 0.19 | 0.19      |

## Installation

```toml
[dependencies]
bevy     = "0.19"
bevy_ufbx = "0.19"
```

## Feature flags

| feature | default | effect |
|---------|---------|--------|
| `animation` | enabled | `AnimationClip`s (TRS + morph `WeightsCurve`), `AnimationPlayer` / `AnimationTargetId` scene wiring, `Animation{N}` / `AnimationRest` labels, `Fbx.animations` / `Fbx.named_animations`, `FbxNode.is_animation_root` |
| `pbr_multi_layer_material_textures` | **off** | Forwards Bevy's opt-in feature of the same name: compiles in `StandardMaterial::clearcoat_texture` / `clearcoat_roughness_texture` / `clearcoat_normal_texture` (+ `_channel` fields) so FBX coat maps (`pbr.coat_factor`, `pbr.coat_roughness`, `pbr.coat_normal`) can bind. Off by default exactly like `bevy` / `bevy_gltf`, where the feature is not in `default`; without it the coat *scalar* factors still apply, only the textures are skipped. |

Without `animation` the crate builds and still loads meshes, materials, skins, hierarchy, lights, and cameras — but no clips, no `Animation{N}` labels, and no animation-target components. `FbxLoaderSettings::load_animations`, `bake_fps`, and `generate_rest_animation` are then inert. (CI enforces a warning-free lib build under `RUSTFLAGS="-D warnings"`.)

## Quick start

```rust
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use bevy_ufbx::FbxPlugin;

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(Camera3d::default());
    commands.spawn(WorldAssetRoot(
        asset_server.load("character.fbx#Scene0"),
    ));
}
```

## Animation

Requires the default `animation` feature.

Animation stacks load into labeled `AnimationClip`s (TRS + morph `WeightsCurve`), baked with
`ufbx::bake_anim`. **Cubic-spline channels are emitted as exact curves** when ufbx's
evaluator can verify them: `CubicKeyframeCurve` / `CubicRotationCurve` (the same types `bevy_gltf` uses
for CUBICSPLINE) built from per-key derivatives sampled through `ufbx_evaluate_transform`, with a
verification probe against ufbx at four interior points of **every segment** — any mismatch silently
falls back to the baked track, so correctness never depends on the exact path succeeding.
Stepped/hold keys render as holds (not ramps), rotation keys are made antipodally continuous, and a curve
that fails to build warns with its node path instead of vanishing.
Morph-weight curves use a wide Hermite path inside `WeightsCurve` rather than bevy_animation's
`WideCubicKeyframeCurve` — the Bevy 0.19.1 type has known defects (panic when sampled at a key time;
swapped end arguments in the between-key helper, pinned by probe tests in `tests/animation_cubic.rs`),
so the loader ships the documented semantics itself; this is a workaround around an upstream bug, not a
loader limitation.
`bake_fps` (default **30.0**, `BakeOpts::resample_rate`) still controls the fallback bake rate and
morph-weight sample density. The loader does **not** auto-play — build an `AnimationGraph` in your app.

Optional rest/bind clip: set `generate_rest_animation: true` to also emit a clip labeled
`AnimationRest` / named `"Rest"` with a single keyframe at `t = 0` for each animated node's
rest local transform (and morph weights when present). Useful for character workflows
(Godot-style rest pose); off by default so existing take indices stay unchanged.

Clips are labeled `Animation{N}` only (Bevy cannot attach two labels to one asset). Look up takes by name on the loaded `Fbx`:

```rust
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxLoaderSettings};

// By index label:
let clip = asset_server.load(FbxAssetLabel::Animation(0).from_asset("cube_anim.fbx"));

// Optional rest/bind (requires generate_rest_animation on load):
// let rest = asset_server.load(FbxAssetLabel::AnimationRest.from_asset("character.fbx"));

// By take name (after the Fbx asset is loaded):
// let clip = fbx.named_animations["Take 001"].clone();
// let rest = fbx.named_animations["Rest"].clone();

let (graph, index) = AnimationGraph::from_clip(clip);
let graph_handle = graphs.add(graph); // ResMut<Assets<AnimationGraph>>
// Then spawn/attach: AnimationPlayer (loader puts it on the armature root),
// insert AnimationGraphHandle(graph_handle), player.play(index).repeat();
```

```rust
// Enable rest clip + custom bake rate:
let fbx: Handle<Fbx> = asset_server
    .load_builder()
    .with_settings(|s: &mut FbxLoaderSettings| {
        s.bake_fps = 30.0;
        s.generate_rest_animation = true;
    })
    .load("character.fbx");
```

## Units / coordinate conversion

Default `convert_coordinates: true` loads into Bevy RH Y-up **metres**.
`FbxSpaceConversion::Auto` (default) picks `ModifyGeometry` for Maya/cm-style files and
`AdjustTransforms` for Blender exports, per [ufbx coordinate-space guidance](https://ufbx.github.io/elements/nodes/#coordinate-spaces).
`TransformRoot` spawns a wrapper entity above the scene (the synthetic ufbx root is not a real FBX node), so
top-level nodes keep the requested conversion without entering animation name paths.

## Supported

- Triangle meshes: position, normal, UV0/UV1, vertex color, tangents (bitangent-handedness sign derived
  from `vertex_bitangent`; shared mesh `element_id`s reused)
- Morph / blend shapes → `MorphWeights` + `WeightsCurve`
- Skinned meshes (LBS, top-4 weights, IBM `Skin{i}/InverseBindMatrices`, joint cap 256): weights come from
  ufbx per-vertex tables (influence-sorted, renormalized); a bad cluster warns and is skipped instead of
  failing the file. One skin deformer per mesh — extras warn. `JOINT_INDEX`/`JOINT_WEIGHT` attributes are
  written only when a skin instance actually references the mesh (glTF-parity guard).
- PBR materials: per-slot color space (color slots sRGB, data slots linear, glTF-parity; dual-published as
  `Texture{N}` / `Texture{N}/Linear`), per-map UV channel (`*_channel`), load-time packing (opacity luminance
  mask → base-color alpha; separate metallic/roughness → G/B) for embedded **and pre-read external** textures,
  external reference resolution (adjacent basename / `.fbm`), wrap modes (sampler precedence:
  `override_sampler` > per-load `default_sampler` > plugin-level `DefaultFbxImageSampler` resource
  > built-in default)
- Specular / anisotropy / clearcoat: `pbr.specular_factor` → `reflectance` (×0.5, the `bevy_gltf`
  formula), `specular_color` → `specular_tint`, `specular_anisotropy`/`specular_rotation` →
  `anisotropy_strength`/`anisotropy_rotation`, coat → `clearcoat`/`clearcoat_perceptual_roughness`;
  coat textures bind behind the default-off `pbr_multi_layer_material_textures` feature. Plus
  transmission / IOR factors when present; Blender PBR flag when exporter is Blender
- Hierarchy `WorldAsset`; `FbxNode` children + skin. Scene root carries `Name` + `FbxSceneName` +
  `FbxSceneExtras` (visible root — the old hidden marker entity is gone, see Known limitations).
  The first camera in scene order is spawned with `Camera { is_active: true }` (glTF parity).
  Lights get `range` from the FBX attenuation-end raw property (fallback 20.0 = `KHR_lights_punctual`
  default); `SpotLight.radius` mirrors `range`. Mesh entities get an explicit `Aabb` at load time;
  primitive entities get `Name` `"{mesh}.{material}"` and `FbxPrimitive.name` is populated.
- Granular `FbxError` (`#[non_exhaustive]`, 10 variants incl. structured `UfbxLoad { path, message }` and
  `MorphTargets { mesh, message }`); texture decode failure stays warn + fallback, never a load error.
- NURBS surfaces tessellated to triangle meshes
- USER_DEFINED custom props as `FbxExtras`
- Constraint / dual-quaternion: load with warning (bake in DCC)

Skin inverse bind matrices are ufbx's canonical `cluster.geometry_to_bone` — the documented inverse bind
matrix ([ufbx deformers guide](https://ufbx.github.io/elements/deformers/)). Earlier versions computed
`bind_to_world⁻¹ · geometry_to_world`; ufbx builds `geometry_to_world` from the bone's **load-time** pose,
so that form carried a stray `bind⁻¹ · bone-load-pose` factor whenever the file's load pose differs from its
bind pose (corpus-measured up to 2.97 relative error) and could be non-finite for singular bind poses.

## Capability parity vs `bevy_gltf` 0.19.1

Post-fix state (2026-09-29 parity pass). Verdicts: **Parity** — functionally equivalent to
`bevy_gltf`; **Parity+** — superset; **Locked** — cannot be closed inside this crate without
modifying Bevy itself or the ufbx parser (listed under Engine-locked limits / Engine-impossible).

| Area | bevy_gltf 0.19.1 | bevy_ufbx (this crate) | Verdict |
|------|------------------|------------------------|---------|
| Asset + labels (`Scene{N}`, `Mesh{N}/Primitive{P}`, `Material{N}`, `Texture{N}`, `Animation{N}`, `Skin{N}`, …) | `Gltf` + `GltfAssetLabel` | `Fbx` + `FbxAssetLabel` (same set + `axis_system`, `unit_scale`, `metadata`, `AnimationRest`) | Parity+ |
| Specular / anisotropy / clearcoat scalars | from KHR extensions | `reflectance = specular_factor×0.5`, `specular_tint`, `anisotropy_strength`/`rotation`, `clearcoat`/`clearcoat_perceptual_roughness` from ufbx PBR maps (authored only) | Parity |
| Clearcoat textures | behind `pbr_multi_layer_material_textures` | same-named default-off feature → `clearcoat_texture` / `clearcoat_roughness_texture` / `clearcoat_normal_texture` | Parity |
| Texture color space / packing / pre-read externals | sRGB vs linear by glTF usage | dual-published `Texture{N}` / `Texture{N}/Linear`, opacity + metal/rough packs, pre-read external bytes | Parity+ |
| Default sampler resource | `DefaultGltfImageSampler` | `DefaultFbxImageSampler`, same API; precedence `override_sampler` > per-load `default_sampler` > resource > built-in default | Parity |
| Sampler mag/min/mip filters | from glTF sampler | not available — ufbx 0.9 typed API exposes only wrap modes | **Locked** (parser) |
| Compressed image formats | `CompressedImageFormatSupport` read in `GltfPlugin::finish` | same pattern via `FbxPlugin::finish` → `FbxCompressedImageFormatSupport` + load-time cache; decode failure = warn + fallback (bevy_gltf fails the load) | Parity+ (deliberate) |
| Animation: linear / step / constant | uneven, stepped, constant curves | same + step-aware baked curves | Parity |
| Animation: cubic-spline TRS | `CubicKeyframeCurve` / `CubicRotationCurve` | exact curves from ufbx-evaluated keys, per-segment verified against ufbx, silent baked fallback | Parity |
| Morph weight curves | `WideCubicKeyframeCurve` (has 2 known defects in bevy_animation 0.19.1) | private wide Hermite curve with the documented semantics inside `WeightsCurve` | Parity (upstream workaround) |
| Active camera | first camera in scene order | same | Parity |
| Light `range` / spot `radius` | `range` default 20.0, spot radius mirrors range | `range` from FBX attenuation-end raw props, fallback 20.0; spot radius mirrors range | Parity |
| Light intensity units | lux / candela (KHR_lights_punctual) | documented heuristic multipliers — FBX `Intensity` is DCC-relative, **not** physical units | Locked (documented heuristic) |
| Scene root components | `Name` + `GltfSceneName` + `GltfSceneExtras` on visible world root | same on visible root (`FbxSceneName`/`FbxSceneExtras`); hidden marker entity removed | Parity (behavior change) |
| Explicit `Aabb` on mesh entities | yes | yes (load-time; NURBS-tessellated nodes keep runtime calc) | Parity |
| Primitive `Name` / `*.name` | `"{mesh}.{material}"` | `FbxPrimitive.name` + `Name` via `FbxPrimitive::name_for` | Parity |
| `JOINT_INDEX`/`JOINT_WEIGHT` guard | warn + skip on non-skinned meshes | written only when a skin instance references the mesh | Parity |
| Error type | granular `GltfError` | granular `#[non_exhaustive]` `FbxError` (10 variants, structured context) | Parity |
| `EXT_mesh_gpu_instancing` | **not supported** (extension table ❌) | not supported; runtime parity = N entities sharing `Handle<Mesh>`/`Handle<StandardMaterial>` (shared `element_id`s already reuse one `FbxMesh`) | Parity |
| Parsed document in `source` | `Option<gltf::Gltf>` | raw bytes + `FbxMeta` — `ufbx::Scene` is not `Send` | **Locked** (architecture) |
| Non-triangle topologies | points/lines/strips supported | triangle list only (FBX polygonal geometry is triangulated) | N-A for FBX |
| KHR sheen / iridescence / dispersion / variants / meshopt / draco / basisu / webp / gpu_instancing | not honored by Bevy's glTF path either | no FBX equivalent expected | Parity (both absent) |

## Known limitations (current)

Docs say what the loader does **today** — this is Bevy-glTF-class parity, not a complete Autodesk FBX solver,
and no claim of full support for arbitrary FBX files.

- **Untrusted input**: ufbx is a native parser; malformed FBX data can abort the host process. This loader
  targets DCC-exported assets — do not feed untrusted FBX to a process expecting robust error recovery.
  (The corpus test isolates hostile files. `load_external_files: false` on every ufbx parse also disables
  ufbx's unmanaged C file I/O — cache / `.mtl` side-loads never happen. The verified corpus sweep still
  contains 2 files that crash the native parser (native terminations 0xC0000409 / 0xC0000005, isolated in
  the corpus test).)
- **One UV transform per material**: Bevy 0.19 `StandardMaterial` carries a single `uv_transform`, so
  per-map transforms collapse to one (base-color) transform. Per-map **channel** selection is supported
  (`*_channel: UvChannel`).
- **External texture resolution**: absolute DCC paths reduce to the adjacent basename or the `.fbm` folder;
  candidate paths are clamped so escaping reads never happen. Texture bytes the loader cannot read are
  warned and omitted from packing rather than bound into the wrong channel.
- **Duplicate node/mesh names** overwrite each other in `named_*` maps and can collide `AnimationTargetId`
  paths (bevy_gltf has the same limitation).
- **Light intensity is a documented heuristic, not a physical unit conversion.** FBX `Intensity` is a
  DCC-relative factor — it is **not** lux or candela, and the loader does not pretend otherwise (unlike
  `bevy_gltf`, which passes through KHR_lights_punctual lux/candela). The fixed multipliers
  (directional ×10000, point/spot ×1000) exist only to preserve existing visuals; area lights become
  point lights. Light `range` **is** set: from the FBX attenuation-end raw property
  (`FarAttenuationEnd`/`AttenuationEnd`/`DecayStart` on `ufbx_light.props`), falling back to 20.0
  (`KHR_lights_punctual` default) when the file authors no usable value; `SpotLight.radius` mirrors
  `range`. Spot cones are converted correctly (FBX full-aperture degrees → Bevy half-angle radians;
  missing → 45°, clamped to 160°).
- **Emission color** is treated as linear.
- **GPU instancing**: `EXT_mesh_gpu_instancing` metadata is not imported — **and `bevy_gltf` does not
  support that extension either** (Bevy's glTF extension table marks it ❌), so there is no parity gap
  here. Runtime parity is the Bevy model itself: N entities sharing `Handle<Mesh>` /
  `Handle<StandardMaterial>` (shared ufbx mesh `element_id`s already reuse one `FbxMesh`); render-side
  batching is the engine's job.
- **Scene root (behavior change)**: `FbxSceneName` / `FbxSceneExtras` / `Name` now sit on the single
  visible scene root that parents the hierarchy; the old hidden marker entity was removed. Queries that
  expected a hidden sibling entity must target the visible root instead.
- **Compressed containers**: embedded DDS/KTX2 decode consults the render device's
  `CompressedImageFormatSupport` (recorded by `FbxPlugin::finish`) and requires the matching Bevy image
  features (the default feature set ships PNG and JPEG decoders); format detection sniffs magic bytes.
  A decode failure warns and falls back to a plain external reference — it never fails the load
  (bevy_gltf fails the load; this is a deliberate superset).
- **Corpus sweep (wave-3 verification run, `target/l5-corpus` procedure)**: 714 files — 702 loaded,
  10 expected failures, 2 hostile files (native parser terminations, isolated in subprocesses: fatal
  error 0xC0000409, access violation 0xC0000005); 0 unexpected failures, 0 timeouts, 0 panics — zero
  delta vs the pre-parity sweep.
  The 2 hostile files remain a native-parser risk (see Untrusted input). Full post-fix suite
  (`cargo test -- --test-threads=1`): **163 passed / 0 failed**; with `pbr_multi_layer_material_textures`:
  **165 / 0**. Examples check, `-D warnings` feature-off and default lib checks, and rustdoc in both
  modes are clean (`wave3-verify.md`).

See [NOTES.md](NOTES.md) for the full ledger (engine-locked / engine-impossible features, unit/axis conversion, fixtures).

## Engine-impossible (documented, not implemented as live solvers)

- Live FBX constraints / IK
- Dual-quaternion skinning (approximated as LBS)
- Native NURBS renderer (tessellate only)
- Stereo cameras

## Engine-locked limits (cannot be closed without modifying Bevy or ufbx)

Standing constraint for this fork: only what is implementable **without engine changes**. Everything
below is a limit of Bevy 0.19's runtime contracts, of `bevy_gltf`'s own gaps, or of the ufbx 0.9
parser — `bevy_gltf` hits the identical wall, so none of it is a parity gap:

- **One `uv_transform` per material** — `StandardMaterial` carries a single transform; per-map
  transforms collapse to the base-color one (`bevy_gltf` warns on divergent transforms for the
  same reason). Per-map `*_channel` selection is supported.
- **No normal-map scale / occlusion strength** — `StandardMaterial` has no such fields;
  `bevy_gltf` leaves both as TODO too.
- **`MAX_JOINTS = 256`, `JOINT_INDEX` as `Uint16x4`, 4 weights per vertex, LBS only** — engine
  limits shared with `bevy_gltf`; no dual-quaternion skinning in `bevy_render`.
- **`MAX_MORPH_WEIGHTS`** — engine limit on `MorphWeights` (this crate warns + trims safely;
  `bevy_gltf` fails the load).
- **UV0/UV1 only** — `MeshUVs` limit; higher UV sets fall back to UV0.
- **Single normal map per material domain** — one `normal_map_texture` slot for the main layer
  (the clearcoat layer has its own separate `clearcoat_normal_texture`, behind the default-off
  feature).
- **ufbx 0.9 typed API has no mag/min/mip filter fields** — samplers carry FBX wrap modes only;
  filters come from `default_sampler`/`override_sampler`. Raw FBX props would require bypassing
  the parser.
- **`ufbx::Scene` is not `Send`** — no parsed document can live in an asset; `Fbx.source` keeps
  raw bytes + `FbxMeta` instead (same reason `bevy_gltf` can store `gltf::Gltf` and we cannot).
- **Constraints / IK / stereo / audio** — outside Bevy's runtime model (no solver, no audio
  pipeline in scope); warned and skipped, or baked in the DCC.
- **2 corpus fixtures abort the native ufbx parser** — isolated and expected (see Untrusted
  input); fixable only by a ufbx upgrade.
- **KHR extensions Bevy's glTF path does not honor either**: sheen, iridescence, dispersion,
  variants, meshopt compression, draco, `EXT_mesh_gpu_instancing` (and basisu/webp only as raw
  formats, not extension syntax). Not adopting them here creates no gap versus `bevy_gltf`.

## Asset labels

| Label | Type |
|-------|------|
| `Scene{N}` | `WorldAsset` |
| `Mesh{N}` | `FbxMesh` (container) |
| `Mesh{N}/Primitive{P}` | Bevy `Mesh` |
| `Material{N}` | `FbxMaterial` (`.material` → `StandardMaterial`) |
| `Material{N}/Standard` | `StandardMaterial` (non-inverted) |
| `Material{N} (inverted)` | `StandardMaterial` (cull-flipped) |
| `Animation{N}` | `AnimationClip` (requires `animation` feature) |
| `AnimationRest` | `AnimationClip` (rest/bind; only with `animation` feature + `generate_rest_animation`) |
| `Node{N}` | `FbxNode` |
| `Skin{N}` | `FbxSkin` |
| `Skin{N}/InverseBindMatrices` | `SkinnedMeshInverseBindposes` |
| `Texture{N}` | `Image` (embedded or pre-read external; sRGB variant) |
| `Texture{N}/Linear` | `Image` (linear variant, when one map feeds both slot kinds) |
| `Material{i}/BaseColorOpacity` | `Image` (packed base color + opacity luminance mask in alpha) |
| `Material{i}/MetallicRoughness` | `Image` (packed, G = roughness, B = metallic) |
| `DefaultMaterial` | `StandardMaterial` |

## Examples

GUI launcher (lists every example, optional args, log, Stop):

```sh
python run_examples_gui.py
```

Visual demos (each proves a runtime feature you can see):

```sh
cargo run --example morph_fbx              # blend shapes (opaque Maya Lambert)
cargo run --example animated_mesh_fbx      # baked TRS take via AnimationGraph
cargo run --example skinned_mesh_fbx       # LBS skin + joint anim (sausage / rigged_triangle)
cargo run --example nurbs_fbx              # NURBS tessellated to Mesh, spinning
cargo run --example static_mesh_fbx        # hierarchy + materials + units (Suzanne / cube)
cargo run --example multimaterial_fbx      # material-split primitives (color regions)
cargo run --example nested_meshes_fbx      # nested mesh hierarchy (cube/cone/ico/plane)
cargo run --example textures_fbx           # embedded textures + wrap
cargo run --example materials_pbr_fbx      # PBR scalar parity: specular reflectance, anisotropy, clearcoat
cargo run --example sampler_settings_fbx   # texture-sampler precedence chain (4 tiers, see below)
cargo run --example lights_cameras_fbx     # FBX DirectionalLight + activated Camera3d
cargo run --example vertex_color_fbx       # Mesh ATTRIBUTE_COLOR (ZBrush painted verts)
cargo run --example neg_scale_fbx          # mirrored mesh → Material{N} (inverted) cull
cargo run --example showcase_fbx           # morph | anim | nurbs | skin | multi-mat
cargo run --example load_fbx               # generic CLI loader (defaults to cube.fbx)
cargo run --example load_fbx -- cube.fbx
cargo run --example dump_fbx -- cube.fbx   # full headless introspection dump (see below)
```

Example notes for the three newest entries:

- `materials_pbr_fbx` — PBR scalar parity: ufbx `specular_factor × 0.5` → `reflectance`,
  `specular_color` → `specular_tint`, `specular_anisotropy`/`specular_rotation` →
  `anisotropy_strength`/`anisotropy_rotation`, coat → `clearcoat`/`clearcoat_perceptual_roughness`.
  The clearcoat **texture** slots (`clearcoat_texture` / `clearcoat_roughness_texture` /
  `clearcoat_normal_texture`) demonstrate only when built with
  `--features pbr_multi_layer_material_textures` — the `StandardMaterial` fields compile out
  without it, and the example prints which variant it was built with.
- `sampler_settings_fbx` — texture-sampler precedence, strongest first:
  `FbxLoaderSettings::override_sampler` > `FbxLoaderSettings::default_sampler` >
  `DefaultFbxImageSampler` resource > built-in default (base matches `bevy_gltf`'s
  `ImageSamplerDescriptor::linear()`). Four instances of one FBX side by side, each tier winning
  over everything below it.
- `dump_fbx` — full headless introspection dump: prints every loader output — asset-label lists
  and `FbxAssetLabel` resolution, `FbxMesh` / primitive names, scene-root components
  (`Name` / `FbxSceneName` / `FbxSceneExtras`), per-entity `Aabb`, camera `is_active` activation,
  light `range` / `radius`, `StandardMaterial` scalars and textures, `FbxExtras` blobs, and — on
  failure — the granular `FbxError`.

## License

MIT OR Apache-2.0
