use std::fs;
use std::io::ErrorKind;
use std::sync::Arc;

use artist_kernel::Kernel;
use artist_vfs::mount;

#[test]
fn mounts_kernel_read_only() {
    let kernel = Kernel::new();

    let tmp = tempfile::tempdir().unwrap();
    let mountpoint = tmp.path().join("mnt");
    fs::create_dir(&mountpoint).unwrap();

    let session = mount(Arc::new(kernel), &mountpoint, true).expect("mount should succeed");

    let names: Vec<String> = fs::read_dir(&mountpoint)
        .expect("read_dir should succeed")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["url"]);

    let resource_dir = mountpoint.join("url");
    let attrs = fs::metadata(&resource_dir).expect("metadata should succeed");
    assert!(attrs.is_dir());
    assert!(
        fs::read_dir(&resource_dir)
            .expect("read_dir")
            .next()
            .is_none()
    );

    let write = fs::File::create(resource_dir.join("nope"));
    let err = write.expect_err("writing to a read-only fs should fail");
    assert_eq!(err.kind(), ErrorKind::ReadOnlyFilesystem);

    session.umount_and_join().expect("umount should succeed");
}
