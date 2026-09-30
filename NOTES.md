# bevy_ufbx NOTES

Forked loader in `tir_refs/fbx` aiming at **Bevy glTF-class FBX parity** (Bevy 0.19 + ufbx 0.9).

## Parity bar

Match what `bevy_gltf` exposes at runtime, fed by ufbx — **not** 1:1 Autodesk FBX fidelity. There is no
complete Autodesk FBX solver here: no live IK/constraint/cache-deformer solving, no dual-quaternion skinning,
no character presets, no audio. Anything Bevy cannot consume at runtime is either baked (`bake_anim` TRS),
tessellated (NURBS), approximated (DQuat → LBS), or warned about and skipped. Docs (README/NOTES) state
current semantics honestly, including remaining correctness gaps.

## Implemented

- Hierarchy `WorldAsset`, skins + IBM, bake_anim TRS + morph `WeightsCurve`
- Mesh attrs: color, tangent with derived bitangent-handedness sign (or generated), UV0/UV1
- Shared mesh instances: same ufbx mesh `element_id` on multiple nodes reuses one `FbxMesh` / primitives
- Blend shapes → morph targets + `MorphWeights` / `MeshMorphWeights`
- Embedded + **pre-read external** textures (async `read_asset_bytes` before material processing; `load_external_files: false` disables ufbx unmanaged C I/O); wrap modes; richer PBR factors (coat/transmission/IOR)
- NURBS → tessellate to `Mesh`
- Constraint / non-linear skinning: warn and continue
- Skin weights from ufbx per-vertex tables (`skin_deformer.vertices[]/weights[]`): top-4, influence-sorted,
  renormalized; a bad cluster warns and is skipped instead of failing the load (see Skin section above).
  Public helper API preserved through review: top-4 weights keeps its 2 args; `bind_unweighted_vertex`
  public helper applies a slot-0 fallback with a warning.
- Animation: stepped keys → holds, antipodal rotation continuity, failed curves warn with node path + track
- Spot lights: FBX full-aperture degrees → Bevy half-angle radians (missing → 45°, clamp 160°), unit-tested
- `TransformRoot` space conversion spawns a synthetic-root wrapper entity so top-level nodes keep the requested conversion (wrapper excluded from animation name paths)
- Real-loader corpus sweep test (`UFBX_TEST_DATA`, hostile files isolated) + parity contract tests
- Honest `axis_system` / `unit_scale` when `convert_coordinates: false`
- `FbxExtras` from **USER_DEFINED** custom props only (built-in / synthetic props skipped)
- `FbxMeshName` components
- `FbxMesh` container asset (glTF-style): `Mesh{N}` labels `FbxMesh`; Bevy meshes are `Mesh{N}/Primitive{P}`
- `FbxPrimitive` on `FbxMesh.primitives` (mesh + optional `StandardMaterial` + extras); `Fbx.primitive_meshes` flat Bevy [`Mesh`] list; `FbxNode.mesh` is `Option<Handle<FbxMesh>>`
- `FbxNode`: `children` + `skin` handles filled (two-pass label reservation, glTF-style)
- `FbxMaterial` container asset (glTF-style): `Material{N}` → `FbxMaterial` (`.material` is `StandardMaterial`); inverted twins stay `Material{N} (inverted)` as bare `StandardMaterial`
- World **neg-scale**: odd count of negative axes on **world** scale (not local `sx*sy*sz`) selects `Material{N} (inverted)` cull twin (skips double-sided)
- Texture samplers: FBX `wrap_u` / `wrap_v` → address modes; mag/min/mip from `FbxLoaderSettings::default_sampler` (or full replace via `override_sampler`) — ufbx 0.9 has no filter fields. Plugin-level default: `DefaultFbxImageSampler` resource; precedence `override_sampler` > per-load `default_sampler` (when explicitly set) > resource > built-in default (base matches `bevy_gltf`'s `ImageSamplerDescriptor::linear()`); see `sampler_settings_fbx`
- Blender PBR: `use_blender_pbr_material` when probe exporter is Blender binary/ascii
- Anim takes: `Animation{N}` labels + `Fbx.named_animations["Take 001"]` map (no second name label)
- Dual-published texture color space labels: `Texture{N}` (sRGB) / `Texture{N}/Linear`; packed labels
  `Material{i}/BaseColorOpacity` + `Material{i}/MetallicRoughness`
- `bake_fps` (default 30) → `BakeOpts::resample_rate` + morph sampling; optional `generate_rest_animation` → `AnimationRest` / `"Rest"`
- `--no-default-features` lib build: CI job enforces it with `RUSTFLAGS: "-C debuginfo=0 -D warnings"` +
  `cargo check --lib --no-default-features` (plain cargo args — warning enforcement via env, keeping the
  shared `debuginfo=0` flag; node.rs/scene.rs cfg-gating landed by L2/L4)

### glTF-parity pass (2026-09-29, waves L-MAT/L-PLUGIN/L-ANIM/L-TEX/L-SCENE-2/L-MESH-ERR)

- **Specular/anisotropy/clearcoat scalars → `StandardMaterial`**: `pbr.specular_factor` →
  `reflectance = factor × 0.5` (exact `bevy_gltf` formula), `specular_color` → `specular_tint`,
  `specular_anisotropy`/`specular_rotation` → `anisotropy_strength`/`anisotropy_rotation`, coat →
  `clearcoat`/`clearcoat_perceptual_roughness` — all applied only when the ufbx map is authored
  (`has_value`). Specular magnitude for non-glTF shading models (Phong/Arnold/Max Physical) approximates
  the glTF scale rather than matching it exactly — flagged, not faked.
- **Clearcoat textures**: `coat_factor` → `clearcoat_texture`, `coat_roughness` →
  `clearcoat_roughness_texture`, `coat_normal` → `clearcoat_normal_texture` (all linear), gated on the
  default-off **`pbr_multi_layer_material_textures`** Cargo feature (forwards `bevy/…`; same gate
  `bevy`/`bevy_gltf` use — the `StandardMaterial` texture fields only exist when bevy_pbr compiles them
  in). `coat_glossiness` is deliberately not bound (gloss-inverting sampler doesn't exist; same
  precedent as the main roughness map).
- **Plugin-level sampler + compressed formats**: `DefaultFbxImageSampler` resource (same API as
  `DefaultGltfImageSampler`) with precedence `override_sampler` > per-load `default_sampler` >
  resource > built-in default;
  the loader is registered in `FbxPlugin::build` sharing the resource's `Arc<Mutex<…>>`.
  `FbxPlugin::finish()` reads the render device's `CompressedImageFormatSupport` into
  `FbxCompressedImageFormatSupport` + a process-wide cache that texture code consumes (headless runs
  without a finished plugin see `NONE`); decode failure stays warn + fallback.
- **Exact cubic animation curves**: cubic-authored TRS channels become `CubicKeyframeCurve` /
  `CubicRotationCurve` built from per-key derivatives sampled through `ufbx_evaluate_transform`, with a
  per-segment probe against ufbx at s ∈ {0.2, 0.4, 0.6, 0.8}; any mismatch silently falls back to the
  baked track (stepped/linear/constant/multi-layer/scale-helper paths unchanged). Morph weights use a
  private wide Hermite curve inside `WeightsCurve` because bevy_animation 0.19.1's
  `WideCubicKeyframeCurve` has two confirmed defects (panic on the exact-key branch; swapped end
  arguments in the between-key helper) — probe tests in `tests/animation_cubic.rs` document the
  upstream bug and mark the switch-back point.
- **Scene parity**: first camera in scene order spawns with `Camera { is_active: true }`; light `range`
  from FBX attenuation-end raw props (`FarAttenuationEnd`/`AttenuationEnd`/`DecayStart`) with fallback
  20.0 (KHR_lights_punctual default), `SpotLight.radius` mirrors `range`; **scene-root merge** — visible
  root now carries `Name` + `FbxSceneName` + `FbxSceneExtras` and parents the hierarchy, hidden marker
  entity removed (**deliberate behavior change**); explicit `Aabb` on mesh entities (NURBS-tessellated
  nodes keep runtime calc); primitive entities get `Name` `"{mesh}.{material}"`.
- **Mesh/errors**: `JOINT_INDEX`/`JOINT_WEIGHT` written only when a skin instance references the mesh
  (`skin_instanced_mesh_elements` predicts exactly which `process_skins` emits for); `FbxPrimitive.name`
  populated from `FbxPrimitive::name_for`; `FbxError` is `#[non_exhaustive]` with 10 variants incl.
  structured `UfbxLoad { path, message }` and `MorphTargets { mesh, message }`.

## Skin inverse bind matrices (canonical `geometry_to_bone`)

`src/node.rs` reads the inverse bind matrix **directly from `cluster.geometry_to_bone`** — ufbx documents it
as the transform from mesh geometry space to local bone space, "often referred to as the inverse bind matrix"
([ufbx deformers guide](https://ufbx.github.io/elements/deformers/)).

Correction history (do not reintroduce the old form): the loader previously computed
`bind_to_world⁻¹ · geometry_to_world` on the assumption that ufbx sets
`geometry_to_world = bind_to_world · geometry_to_bone`. ufbx actually computes
`geometry_to_world = bone_node.node_to_world · geometry_to_bone` (`ufbx.c`, `ufbxi_update_skin_cluster`) —
with the bone's **load-time** pose, not the bind pose. The composed form therefore diverges from the true IBM
whenever a file's load pose differs from its bind pose (L2 corpus: up to 2.97 relative error) and can produce
non-finite matrices for singular bind poses. Reading `cluster.geometry_to_bone` directly is a **correctness
semantic fix**, not a cosmetic simplification.

Skin weights come from ufbx's per-vertex tables (`skin_deformer.vertices[]` / `weights[]`), sorted by
decreasing influence, first 4 taken and renormalized — the recipe documented on the same deformers page.

## Texture semantics (current)

- Slot bindings resolve from ufbx's resolved `MaterialMap.texture` (`pbr.base_color`, `pbr.opacity`,
  `pbr.emission_color`, `pbr.normal_map`, `pbr.metalness`, `pbr.roughness`, `pbr.ambient_occlusion`, plus
  `pbr.coat_factor` / `pbr.coat_roughness` / `pbr.coat_normal` → clearcoat slots behind the default-off
  `pbr_multi_layer_material_textures` feature), with the
  FBX property-name match as fallback. Color slots (base color, emissive, opacity) decode sRGB; data slots
  (normal, metallic, roughness, AO, height, all three coat maps) decode linear — `bevy_gltf` parity. A map feeding both kinds of
  slots is dual-published: `Texture{N}` (sRGB) and `Texture{N}/Linear`.
- Embedded **and pre-read external** images are packed at load time: an opacity **luminance mask** is written
  into the base-color alpha (`Material{i}/BaseColorOpacity`, so `AlphaMode::Mask` works without graying the
  albedo); separate metallic/roughness maps are packed to the glTF layout (`Material{i}/MetallicRoughness`,
  G = roughness, B = metallic). Format detection sniffs magic bytes (PNG/JPEG/DDS/WebP) before trusting the
  filename extension. Verified against on-disk checkerboard PNGs (`blender_293_textures_7400_binary.fbx`):
  sRGB base == diffuse PNG, packed G/B == roughness/metallic PNGs (targeted tests 5/5 on the current tree).
- **Runtime external reads**: the loader pre-reads external texture bytes before processing — send-safe owned
  `(texture id, candidate paths)` requests, scene dropped, `read_asset_bytes().await`, re-parse, then sync
  processing. `load_external_files: false` on all three ufbx parses (probe, main, …): no unmanaged C file
  I/O, so geometry-cache / `.mtl` side-loads never happen. All per-slot color space, UV selection, and
  packing applies **equally to external and embedded**. Caveats: absolute DCC paths reduce to the adjacent
  basename or `.fbm` folder; escaping reads never happen (candidates clamped); missing bytes warn and omit
  packing (no wrong-channel binding). The synchronous compatibility entry (`process_materials` without
  pre-read bytes) remains limited to embedded content.
- Per-map UV channel (`*_channel: UvChannel`) is selected from the texture's `uv_set`. Bevy 0.19 supports
  **one** `uv_transform` per `StandardMaterial`, so per-map transforms collapse to the base-color transform.
- Compressed embedded containers (DDS/KTX2) additionally need the matching Bevy image features enabled
  (the default feature set ships PNG and JPEG decoders). Decoder selection consults the render device's
  `CompressedImageFormatSupport` (recorded by `FbxPlugin::finish` into `FbxCompressedImageFormatSupport`
  + a process-wide cache; headless runs without a finished plugin see `NONE`); a decode failure warns and
  falls back to a plain external reference — never a load error (deliberate superset over `bevy_gltf`,
  which fails the load).
- Packing skips when the maps to combine have mismatched UV wrap modes (one sampler cannot satisfy both).
  The opacity luminance mask is packed from **raw** luminance bytes — authored values are preserved, no
  color management applied at pack time (the mask slot itself decodes sRGB when displayed standalone).
  Alpha mode: scalar opacity factor < 0.98 → `AlphaMode::Blend` (wins over the texture cutout heuristic);
  texture-only opacity → `AlphaMode::Mask(0.5)`.

## Engine-impossible / bake-only

| FBX feature | Behavior |
|-------------|----------|
| Constraints / IK | Warning; bake TRS in DCC / bake_anim |
| Dual-quaternion skin | Warning; LBS approximation |
| Native NURBS | Tessellate only |
| Stereo / character / audio | Ignored |

## Engine-locked (cannot be fixed without modifying Bevy or ufbx)

Scope rule: only changes implementable **without engine modifications**. These limits belong to Bevy
0.19 / `bevy_gltf` / ufbx 0.9 themselves — `bevy_gltf` hits the same wall, so none is a parity gap:

- One `uv_transform` per `StandardMaterial` (per-map transforms collapse to base color; per-map
  `*_channel` works).
- No normal-map scale, no occlusion strength — `StandardMaterial` has no such fields; `bevy_gltf`
  leaves both TODO.
- `MAX_JOINTS = 256`, `JOINT_INDEX` as `Uint16x4`, 4 weights per vertex, LBS only (no
  dual-quaternion in `bevy_render`).
- `MAX_MORPH_WEIGHTS` (this crate trims with a warning; `bevy_gltf` fails the load).
- UV0/UV1 only (`MeshUVs` limit).
- One normal map per material domain (`normal_map_texture`); the clearcoat layer's dedicated
  `clearcoat_normal_texture` exists behind the default-off `pbr_multi_layer_material_textures` feature.
- ufbx 0.9 typed API exposes no mag/min/mip filter fields on textures — only wrap modes; filters come
  from `default_sampler`/`override_sampler`.
- `ufbx::Scene` is not `Send` → no parsed document in the asset; `Fbx.source` holds raw bytes +
  `FbxMeta` (`bevy_gltf` can store `gltf::Gltf`, we cannot store `ufbx::Scene`).
- Constraints/IK, stereo, audio — outside Bevy's runtime model (warn/skip or bake in DCC).
- 2 corpus fixtures abort the native ufbx parser — isolated and expected; only a ufbx upgrade fixes them.
- KHR extensions Bevy's glTF path ignores too: sheen, iridescence, dispersion, variants, meshopt,
  draco, `EXT_mesh_gpu_instancing` (basisu/webp only as raw formats, not extension syntax). Not
  adopting them creates no gap versus `bevy_gltf`.

## Known limitations (current)

- **Untrusted input**: ufbx is a native parser; malformed FBX data can abort the host process. The loader is
  built for DCC-exported assets — **no claim of full support for arbitrary/hostile files** (the corpus test
  isolates hostile files from the test process; the verified corpus sweep still contains 2 files that crash
  the native parser).
- **One `uv_transform` per material** (Bevy 0.19 runtime constraint); per-map **channel** selection is
  supported via `*_channel`. Color space, UV selection, and packing apply equally to external and embedded
  textures (see Texture semantics for the external-read caveats).
- **Duplicate node/mesh names** overwrite each other in `named_*` maps and can collide `AnimationTargetId`
  paths (bevy_gltf has the identical limitation).
- **Light conversion** is a documented heuristic, **not** a physical unit conversion: FBX `Intensity` is a
  DCC-relative factor and is **not** lux or candela (unlike `bevy_gltf`, which passes KHR_lights_punctual
  lux/candela through). Fixed multipliers (directional ×10000, point/spot ×1000) only preserve existing
  visuals; area lights become point lights. `range` **is** set from the FBX attenuation-end raw property
  (`FarAttenuationEnd`/`Far Attenuation End`/`AttenuationEnd`/`DecayStart` on `ufbx_light.props`),
  fallback 20.0 (KHR_lights_punctual default); `SpotLight.radius` mirrors `range`. Spot cones are
  converted correctly (FBX full-aperture degrees → Bevy half-angle radians; missing → 45°, clamped to 160°).
- **Emission color** is treated as linear.
- **GPU instancing**: `EXT_mesh_gpu_instancing` metadata is not imported — and **`bevy_gltf` does not
  support that extension either** (Bevy's glTF extension table marks it ❌), so this is parity, not
  missing loader work. Parity lives at the scene level: N entities sharing `Handle<Mesh>` /
  `Handle<StandardMaterial>` (shared ufbx mesh `element_id`s already reuse one `FbxMesh`); batching is
  the engine's job.
- **Scene root (behavior change, 2026-09-29)**: `FbxSceneName` / `FbxSceneExtras` / `Name` moved onto the
  visible scene root that parents the hierarchy; the previous hidden marker entity was removed. Code
  querying the hidden marker must target the visible root. `FbxSceneExtras` is now always present on the
  root (empty string when the file authors none).
- **Verification** (wave-3 leaf, post-parity, `wave3-verify.md`): full suite `cargo test -- --test-threads=1`
  → **163 passed / 0 failed**; with `pbr_multi_layer_material_textures` → **165 / 0** (lib 46,
  `texture_slots` 6). Corpus sweep (L5 procedure) — **714 files: 702 loaded, 10 expected, 2 hostile
  (isolated subprocesses; ufbx native terminations 0xC0000409 and 0xC0000005), 0 unexpected,
  0 timeouts, 0 panics** — zero delta vs the pre-parity sweep. Per-suite: lib 45, `parity_contract` 16,
  `mesh_error_contract` 6, `animation_cubic` 7, `texture_slots` 5 (6 with the feature), `skin_loading` 9,
  `lambert_opacity` 2. `cargo check --examples`, strict `-D warnings` lib checks (default and
  `--no-default-features`) and rustdoc in both modes all clean. ID-collision empty-stack
  loader bug fixed; collision case 637 ms.

## LoadOpts (when `convert_coordinates`)

RH Y-up, metres. Default [`FbxSpaceConversion::Auto`]: probe exporter with `ignore_all_content`, then
- Blender binary/ascii → `AdjustTransforms` (undo export root ×100)
- otherwise (Maya/cm, Mixamo, unknown) → `ModifyGeometry` (bake units into verts; clean node scales)

Maya centimetre fixtures correctly become ~1 cm in world metres — examples may apply a **demo** root scale for readability.

## Fixtures

Local `assets/` (visual examples + tests):

| File | Proves |
|------|--------|
| `blend_shape_cube.fbx` | morph / blend shapes (`morph_fbx`) |
| `cube_anim.fbx` / `cube_anim_bevy.fbx` | TRS bake (`animated_mesh_fbx`) |
| `cube.fbx` | static mesh / CLI default (`load_fbx`, `static_mesh_fbx` fallback, `dump_fbx` default) |
| `maya_cube_7400_binary.fbx` | Maya cm cube |
| `nurbs_saddle.fbx` | NURBS tessellate (`nurbs_fbx`) |
| `rigged_triangle.fbx` | skin IBM / fallback for `skinned_mesh_fbx` |
| `blender_279_sausage_7400_binary.fbx` | skinned character-ish + takes (`skinned_mesh_fbx`, `showcase_fbx`) |
| `blender_279_internal_textures_7400_binary.fbx` | external checkerboards (mesh has NO UVs) + PBR scalar / sampler-precedence demos (`textures_fbx` LEFT subject, `materials_pbr_fbx`, `sampler_settings_fbx`) |
| `blender_293_textures_7400_binary.fbx` | external checkerboards WITH UVs — visible texture-sampling subject (`textures_fbx` RIGHT); corpus-only, served through the example's router fallback |
| `blender_282_suzanne_7400_binary.fbx` | Blender AdjustTransforms static mesh |
| `blender_279_nested_meshes_7400_binary.fbx` | nested mesh hierarchy (`nested_meshes_fbx`) |
| `blender_suzanne_multimaterial_7400_binary.fbx` | material-split primitives / color regions (`multimaterial_fbx`, `showcase_fbx`) |
| `maya_camera_light_axes_y_up_6100_binary.fbx` | lights + camera activation — first camera in scene order spawns `is_active: true` (no mesh; `lights_cameras_fbx`) |
| `zbrush_vertex_color_7500_ascii.fbx` | vertex colors / `ATTRIBUTE_COLOR` (`vertex_color_fbx`) |
| `blender_340_mirrored_normals_7400_binary.fbx` | neg world scale → inverted cull (`neg_scale_fbx`) |
| `blender340_tangent_sign_7400_binary.fbx` | tangent `w` signs (21 020 bytes, md5 `e8898c0049dbd17c8b621a9d31add642`, verbatim copy of `libs/ufbx/data/…`) |
| `maya_anim_linear_7700_ascii.fbx` | stepped keys → holds (17 157 bytes, md5 `4bcfe71710dbb66f3cba44b729faa723`, verified identical to `libs/ufbx/data/…`) |
| `motionbuilder_lights_7700_ascii.fbx` | real spot-light instances (only corpus file with them; 95 940 bytes, md5 `062f87d244539370d37e79ccec9d8762`, copy of `libs/ufbx/data/…`) |

Copied from `libs/ufbx/data/` where noted; do not invent fixtures.

## Non-goals

- Forking / upstreaming into bevyengine
- Full T.I.R. game character wiring
