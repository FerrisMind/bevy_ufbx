use bevy::prelude::*;
use bevy_ufbx::FbxPlugin;

#[test]
fn test_plugin_builds() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(FbxPlugin);

    // Plugin registered without panic.
    assert!(
        app.world()
            .get_resource::<Assets<bevy_ufbx::Fbx>>()
            .is_some()
    );
}

#[test]
fn test_loader_registration() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(FbxPlugin);

    assert!(
        app.world()
            .get_resource::<Assets<bevy_ufbx::Fbx>>()
            .is_some()
    );
    assert!(
        app.world()
            .get_resource::<Assets<bevy_ufbx::FbxNode>>()
            .is_some()
    );
    assert!(
        app.world()
            .get_resource::<Assets<bevy_ufbx::FbxSkin>>()
            .is_some()
    );
}
