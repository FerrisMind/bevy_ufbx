//! Headless inspection of a loaded FBX asset.
//!
//! Loads the file through the full Bevy asset pipeline (no window) and prints
//! what the loader produced:
//!
//! ```sh
//! cargo run --example dump_fbx -- my_model.fbx
//! ```

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, LoadState};
use bevy::image::Image;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxPlugin};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cube.fbx".to_string());

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .init_asset::<AnimationClip>()
        .init_asset::<WorldAsset>()
        .init_asset::<SkinnedMeshInverseBindposes>()
        .add_plugins(FbxPlugin);

    let handle: Handle<Fbx> = app.world().resource::<AssetServer>().load(path.clone());

    loop {
        app.update();
        match app.world().resource::<AssetServer>().load_state(&handle) {
            LoadState::Loaded => break,
            LoadState::Failed(err) => {
                eprintln!("FAILED to load '{path}': {err}");
                std::process::exit(1);
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }

    let world = app.world();
    let fbx_assets = world.resource::<Assets<Fbx>>();
    let fbx = fbx_assets
        .get(&handle)
        .expect("Fbx asset missing after successful load");

    println!("loaded '{path}'");
    println!("  scenes:    {}", fbx.scenes.len());
    println!("  meshes:    {}", fbx.meshes.len());
    println!("  primitives:{}", fbx.primitive_meshes.len());
    println!("  materials: {}", fbx.materials.len());
    println!("  nodes:     {}", fbx.nodes.len());
    println!("  skins:     {}", fbx.skins.len());
    for name in fbx.named_skins.keys() {
        println!("  skin: {name}");
    }

    let fbx_meshes = world.resource::<Assets<bevy_ufbx::FbxMesh>>();
    for (i, handle) in fbx.meshes.iter().enumerate() {
        let Some(mesh) = fbx_meshes.get(handle) else {
            continue;
        };
        println!(
            "  FbxMesh{i} '{}': {} primitives",
            mesh.name,
            mesh.primitives.len()
        );
        for (p, prim) in mesh.primitives.iter().enumerate() {
            println!(
                "    Primitive{p}: material={}",
                if prim.material.is_some() {
                    "yes"
                } else {
                    "none"
                }
            );
        }
    }

    for (i, (_, mesh)) in world.resource::<Assets<Mesh>>().iter().enumerate() {
        let verts = mesh.count_vertices();
        let joints = mesh
            .attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
            .map(|a| a.len())
            .unwrap_or(0);
        println!("  Mesh{i}: {verts} vertices, {joints} joint indices");
    }

    for (i, (_, mat)) in world
        .resource::<Assets<StandardMaterial>>()
        .iter()
        .enumerate()
    {
        println!(
            "  Material{i}: base_color={:?} textured={}",
            mat.base_color,
            mat.base_color_texture.is_some()
        );
    }
}
