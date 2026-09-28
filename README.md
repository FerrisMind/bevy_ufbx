# bevy_ufbx

FBX asset loader for [Bevy](https://bevyengine.org) powered by [ufbx](https://github.com/ufbx/ufbx).

## Bevy compatibility

| bevy | bevy_ufbx |
|------|-----------|
| 0.19 | 0.19      |
| 0.18 | 0.18      |
| 0.17 | 0.17      |

## Installation

```toml
[dependencies]
bevy     = "0.19"
bevy_ufbx = "0.19"
```

The default feature set includes `animation` (pulls in `bevy_animation`). Disable with `default-features = false` if you only need static meshes.

## Quick start

```rust
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use bevy_ufbx::FbxPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(FbxPlugin)
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(Camera3d::default());
    commands.spawn(WorldAssetRoot(
        asset_server.load("character.fbx#Scene0"),
    ));
}
```

## Animation

Anim stacks are baked with `ufbx::bake_anim` into labeled [`AnimationClip`](https://docs.rs/bevy/latest/bevy/animation/struct.AnimationClip.html)s. The loader does **not** auto-play; build an `AnimationGraph` in your app (see `examples/animated_mesh_fbx.rs`).

```rust
use bevy_ufbx::FbxAssetLabel;

let clip = asset_server.load(FbxAssetLabel::Animation(0).from_asset("cube_anim.fbx"));
let (graph, index) = AnimationGraph::from_clip(clip);
```

Named takes are also on `Fbx.named_animations` when stacks have names.

## Loading with custom settings

```rust
use bevy_ufbx::{Fbx, FbxLoaderSettings};

fn setup(asset_server: Res<AssetServer>) {
    asset_server.load_with_settings::<Fbx, FbxLoaderSettings>(
        "environment.fbx",
        |s| {
            s.load_cameras = false;
            s.load_lights = false;
            s.load_animations = false;
        },
    );
}
```

### `FbxLoaderSettings` fields

| Field                 | Type                | Default                       | Description                                              |
|-----------------------|---------------------|-------------------------------|----------------------------------------------------------|
| `load_meshes`         | `RenderAssetUsages` | `RenderAssetUsages::default()`| Which worlds the mesh is available in                    |
| `load_materials`      | `RenderAssetUsages` | `RenderAssetUsages::default()`| Which worlds the material is available in                |
| `load_cameras`        | `bool`              | `true`                        | Import cameras as inactive `Camera3d`                    |
| `load_lights`         | `bool`              | `true`                        | Import lights onto nodes                                 |
| `load_animations`     | `bool`              | `true`                        | Bake anim stacks into `AnimationClip`s                   |
| `include_source`      | `bool`              | `false`                       | Keep raw bytes on the loaded `Fbx` asset                 |
| `convert_coordinates` | `bool`              | `true`                        | Remap to Bevy RH Y-up metres via ufbx `LoadOpts`         |

## Asset labels

| Label                        | Type                         | Description                          |
|------------------------------|------------------------------|--------------------------------------|
| `Scene{N}`                   | `WorldAsset`                 | Parent hierarchy                     |
| `Mesh{N}`                    | `Mesh`                       | Triangulated mesh                    |
| `Material{N}`                | `StandardMaterial`           | PBR material                         |
| `Animation{N}`               | `AnimationClip`              | Baked take                           |
| `Node{N}`                    | `FbxNode`                    | Transform node                       |
| `Skin{N}`                    | `FbxSkin`                    | Skeletal skin                        |
| `Skin{N}/InverseBindMatrices`| `SkinnedMeshInverseBindposes`| IBM buffer (cluster order)           |
| `DefaultMaterial`            | `StandardMaterial`           | Fallback material                    |

## Supported features

- Triangle meshes (positions, normals, UVs) with multi-material face groups
- PBR materials and textures (including `.fbm` folders)
- Hierarchical `WorldAsset` scenes with `Name`d nodes
- Skinned meshes: top-4 weights, IBM = `inverse(bind_to_world) * geometry_to_world`
- Baked skeletal / transform animation (`AnimationPlayer` / `AnimationTarget` / `AnimatedBy`)
- Directional, point, and spot lights; cameras as inactive `Camera3d`

## Limitations (non-goals)

See [NOTES.md](NOTES.md) for the explicit non-goal list (morph/blend shapes, NURBS, upstream Bevy PR, full game wiring).

## Examples

```sh
cargo run --example load_fbx -- cube.fbx
cargo run --example animated_mesh_fbx
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) at your option.
