use artist_component::resources::{
    DynamicResourcesProvider, ResourceVerbBindings, ResourcesHandler,
};
use artist_kernel::{DynamicValue, Kernel, ResourceUri, VerbId};
use std::{collections::BTreeMap, fs, sync::Arc};

#[tokio::test]
async fn resources_handler_is_available_through_dynamic_resource_registry() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("docs");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("README.md"), "dynamic resource docs\n").unwrap();

    let read = VerbId::new("artist:resource/read@1.0.0").unwrap();
    let handler = Arc::new(ResourcesHandler::new(root.path()).unwrap());
    let provider = DynamicResourcesProvider::new(
        handler,
        ResourceVerbBindings {
            read: read.clone(),
            write: VerbId::new("artist:resource/write@1.0.0").unwrap(),
            edit: VerbId::new("artist:resource/edit@1.0.0").unwrap(),
            insert: VerbId::new("artist:resource/insert@1.0.0").unwrap(),
            poll: VerbId::new("artist:resource/poll@1.0.0").unwrap(),
            run: VerbId::new("artist:resource/run@1.0.0").unwrap(),
            abort: VerbId::new("artist:resource/abort@1.0.0").unwrap(),
            delete: VerbId::new("artist:resource/delete@1.0.0").unwrap(),
            find: VerbId::new("artist:resource/find@1.0.0").unwrap(),
            grep: VerbId::new("artist:resource/grep@1.0.0").unwrap(),
        },
    );
    let kernel = Kernel::new();
    kernel
        .register_dynamic_resource_provider(Arc::new(provider))
        .unwrap();

    let uri = ResourceUri::parse("resources://docs/README.md").unwrap();
    let result = kernel
        .resource_registry()
        .invoke(&read, &uri, DynamicValue::Record(BTreeMap::new()))
        .await
        .unwrap();
    let DynamicValue::Record(fields) = result.output else {
        panic!("resource read did not return a typed record");
    };
    assert_eq!(
        fields.get("uri"),
        Some(&DynamicValue::ResourceUri(uri.clone()))
    );
    let anchor = match fields.get("lines") {
        Some(DynamicValue::List(lines)) => match lines.first() {
            Some(DynamicValue::Record(line)) => line.get("anchor").cloned().unwrap(),
            _ => panic!("resource read returned no anchored line"),
        },
        _ => panic!("resource read returned no lines"),
    };

    let edit = VerbId::new("artist:resource/edit@1.0.0").unwrap();
    let result = kernel
        .resource_registry()
        .invoke(
            &edit,
            &uri,
            DynamicValue::Record(BTreeMap::from([
                ("start".into(), anchor),
                ("end".into(), DynamicValue::Option(None)),
                ("content".into(), DynamicValue::String("edited docs".into())),
            ])),
        )
        .await
        .unwrap();
    assert!(matches!(result.output, DynamicValue::Record(_)));
    assert_eq!(
        fs::read_to_string(package.join("README.md")).unwrap(),
        "edited docs\n"
    );
}
