# bevy_ufbx

FBX asset loader for [Bevy](https://bevyengine.org) powered by [ufbx](https://github.com/ufbx/ufbx).

Targets **Bevy glTF-class runtime parity** for FBX: scenes, skins, baked animation (TRS + morph weights), materials/textures, lights/cameras, NURBS tessellation — everything Bevy can consume. Not a 1:1 Autodesk FBX runtime.

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

Default features include `animation` (pulls `bevy_animation` + `morph_animation`).

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

Stacks are baked with `ufbx::bake_anim` into labeled `AnimationClip`s (TRS + morph `WeightsCurve`).
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
`AdjustTransforms` for Blender exports, per [ufbx coordinate-space guidance](https://ufbx.github.io/docs/nodes/#coordinate-spaces).

## Supported

- Triangle meshes: position, normal, UV0/UV1, vertex color, tangents (shared mesh `element_id`s reused)
- Morph / blend shapes → `MorphWeights` + `WeightsCurve`
- Skinned meshes (LBS, top-4 weights, IBM `Skin{i}/InverseBindMatrices`, joint cap 256)
- PBR materials, embedded textures, `.fbm` paths, wrap modes (+ `default_sampler` / `override_sampler`)
- Clearcoat / transmission / IOR factors when present; Blender PBR flag when exporter is Blender
- Hierarchy `WorldAsset`, lights, inactive cameras; `FbxNode` children + skin
- NURBS surfaces tessellated to triangle meshes
- USER_DEFINED custom props as `FbxExtras`
- Constraint / dual-quaternion: load with warning (bake in DCC)

## Engine-impossible (documented, not implemented as live solvers)

- Live FBX constraints / IK
- Dual-quaternion skinning (approximated as LBS)
- Native NURBS renderer (tessellate only)
- Stereo cameras, GPU mesh instancing

See [NOTES.md](NOTES.md).

## Asset labels

| Label | Type |
|-------|------|
| `Scene{N}` | `WorldAsset` |
| `Mesh{N}` | `FbxMesh` (container) |
| `Mesh{N}/Primitive{P}` | Bevy `Mesh` |
| `Material{N}` | `FbxMaterial` (`.material` → `StandardMaterial`) |
| `Material{N}/Standard` | `StandardMaterial` (non-inverted) |
| `Material{N} (inverted)` | `StandardMaterial` (cull-flipped) |
| `Animation{N}` | `AnimationClip` |
| `AnimationRest` | `AnimationClip` (rest/bind; only if `generate_rest_animation`) |
| `Node{N}` | `FbxNode` |
| `Skin{N}` | `FbxSkin` |
| `Skin{N}/InverseBindMatrices` | `SkinnedMeshInverseBindposes` |
| `Texture{N}` | `Image` (embedded) |
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
