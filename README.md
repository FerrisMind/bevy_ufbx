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
| `animation` | enabled | baked `AnimationClip`s (TRS + morph `WeightsCurve`), `AnimationPlayer` / `AnimationTargetId` scene wiring, `Animation{N}` / `AnimationRest` labels, `Fbx.animations` / `Fbx.named_animations`, `FbxNode.is_animation_root` |

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

Stacks are baked with `ufbx::bake_anim` into labeled `AnimationClip`s (TRS + morph `WeightsCurve`).
Stepped/hold keys render as holds (not ramps), rotation keys are made antipodally continuous, and a curve
that fails to build warns with its node path instead of vanishing.
Sampling rate is controlled by `FbxLoaderSettings::bake_fps` (default **30.0**, maps to
`BakeOpts::resample_rate` and morph-weight sample density). The loader does **not** auto-play —
build an `AnimationGraph` in your app.

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
  failing the file. One skin deformer per mesh — extras warn.
- PBR materials: per-slot color space (color slots sRGB, data slots linear, glTF-parity; dual-published as
  `Texture{N}` / `Texture{N}/Linear`), per-map UV channel (`*_channel`), load-time packing (opacity luminance
  mask → base-color alpha; separate metallic/roughness → G/B) for embedded **and pre-read external** textures,
  external reference resolution (adjacent basename / `.fbm`), wrap modes (+ `default_sampler` / `override_sampler`)
- Clearcoat / transmission / IOR factors when present; Blender PBR flag when exporter is Blender
- Hierarchy `WorldAsset`, lights, inactive cameras; `FbxNode` children + skin
- NURBS surfaces tessellated to triangle meshes
- USER_DEFINED custom props as `FbxExtras`
- Constraint / dual-quaternion: load with warning (bake in DCC)

Skin inverse bind matrices are ufbx's canonical `cluster.geometry_to_bone` — the documented inverse bind
matrix ([ufbx deformers guide](https://ufbx.github.io/elements/deformers/)). Earlier versions computed
`bind_to_world⁻¹ · geometry_to_world`; ufbx builds `geometry_to_world` from the bone's **load-time** pose,
so that form carried a stray `bind⁻¹ · bone-load-pose` factor whenever the file's load pose differs from its
bind pose (corpus-measured up to 2.97 relative error) and could be non-finite for singular bind poses.

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
- **Compressed containers**: embedded DDS/KTX2 decode additionally requires enabling the matching Bevy image
  features (the default feature set ships PNG and JPEG decoders); format detection sniffs magic bytes.
- **Duplicate node/mesh names** overwrite each other in `named_*` maps and can collide `AnimationTargetId`
  paths (bevy_gltf has the same limitation).
- **Light intensity** uses fixed multipliers (directional ×10000, point/spot ×1000) — heuristics, not a
  physical unit conversion; area lights become point lights. Spot cones are converted correctly (FBX
  full-aperture degrees → Bevy half-angle radians; missing → 45°, clamped to 160°).
- **Emission color** is treated as linear.
- **GPU instancing**: dedicated FBX instance metadata is not imported. Not an engine limitation — Bevy
  already auto-batches and shares mesh handles; shared ufbx mesh `element_id`s reuse one `FbxMesh` today.
- **Corpus sweep (verified)**: 714 files — 702 loaded, 10 expected failures, 2 hostile files (native parser
  terminations, isolated in subprocesses: fatal error 0xC0000409, access violation 0xC0000005); 0 unexpected
  failures, 0 timeouts, 0 panics. The 2 hostile files remain a native-parser risk (see Untrusted input).
  Full suite green: **140 passed / 0 failed** (26 lib + 114 integration, incl. 3 corpus tests; doc-tests 0);
  examples check exit 0; feature-off lib check (incl. JPEG) and feature-off rustdoc exit 0 clean.

See [NOTES.md](NOTES.md) for the full ledger (engine-impossible features, unit/axis conversion, fixtures).

## Engine-impossible (documented, not implemented as live solvers)

- Live FBX constraints / IK
- Dual-quaternion skinning (approximated as LBS)
- Native NURBS renderer (tessellate only)
- Stereo cameras

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
cargo run --example lights_cameras_fbx     # FBX DirectionalLight + activated Camera3d
cargo run --example vertex_color_fbx       # Mesh ATTRIBUTE_COLOR (ZBrush painted verts)
cargo run --example neg_scale_fbx          # mirrored mesh → Material{N} (inverted) cull
cargo run --example showcase_fbx           # morph | anim | nurbs | skin | multi-mat
cargo run --example load_fbx               # generic CLI loader (defaults to cube.fbx)
cargo run --example load_fbx -- cube.fbx
cargo run --example dump_fbx -- cube.fbx   # headless asset dump
```

## License

MIT OR Apache-2.0
