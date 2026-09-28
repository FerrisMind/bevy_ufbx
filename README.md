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

Stacks are baked with `ufbx::bake_anim` into labeled `AnimationClip`s (TRS + morph `WeightsCurve`). The loader does **not** auto-play — build an `AnimationGraph` in your app.

```rust
use bevy_ufbx::FbxAssetLabel;

let clip = asset_server.load(FbxAssetLabel::Animation(0).from_asset("cube_anim.fbx"));
let (graph, index) = AnimationGraph::from_clip(clip);
```

## Supported

- Triangle meshes: position, normal, UV0/UV1, vertex color, tangents
- Morph / blend shapes → `MorphWeights` + `WeightsCurve`
- Skinned meshes (LBS, top-4 weights, IBM `Skin{i}/InverseBindMatrices`, joint cap 256)
- PBR materials, embedded textures, `.fbm` paths, wrap modes
- Clearcoat / transmission / IOR factors when present
- Hierarchy `WorldAsset`, lights, inactive cameras
- NURBS surfaces tessellated to triangle meshes
- Custom props as `FbxExtras`
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
| `Mesh{N}` | `Mesh` |
| `Material{N}` | `StandardMaterial` |
| `Animation{N}` | `AnimationClip` |
| `Node{N}` | `FbxNode` |
| `Skin{N}` | `FbxSkin` |
| `Skin{N}/InverseBindMatrices` | `SkinnedMeshInverseBindposes` |
| `Texture{N}` | `Image` (embedded) |
| `DefaultMaterial` | `StandardMaterial` |

## Examples

```sh
cargo run --example load_fbx -- cube.fbx
cargo run --example animated_mesh_fbx
cargo run --example morph_fbx
```

## License

MIT OR Apache-2.0
