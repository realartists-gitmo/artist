#![cfg(any(target_os = "linux", target_os = "freebsd"))]

use gpui::{DmaBufPlane, DmaBufSurface};
use std::{
    fs::File,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn plane() -> DmaBufPlane {
    DmaBufPlane::new(File::open("/dev/null").unwrap().into(), 0, 4)
}

fn callback(counter: Arc<AtomicUsize>) -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
}

#[test]
fn explicit_completion_runs_once_across_clones_and_drop() {
    let completions = Arc::new(AtomicUsize::new(0));
    let surface = DmaBufSurface::new(
        1,
        1,
        875_713_112,
        0,
        vec![plane()],
        Some(callback(completions.clone())),
    );
    let clone = surface.clone();

    surface.complete();
    clone.complete();
    drop(surface);
    drop(clone);

    assert_eq!(completions.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_an_unrendered_frame_releases_it() {
    let completions = Arc::new(AtomicUsize::new(0));
    {
        let _surface = DmaBufSurface::new(
            1,
            1,
            875_713_112,
            0,
            vec![plane()],
            Some(callback(completions.clone())),
        );
    }

    assert_eq!(completions.load(Ordering::SeqCst), 1);
}

#[test]
fn completion_tokens_are_per_present_while_the_image_id_is_stable() {
    let base = DmaBufSurface::new(1, 1, 875_713_112, 0, vec![plane()], None);
    let first = Arc::new(AtomicUsize::new(0));
    let second = Arc::new(AtomicUsize::new(0));
    let first_frame = base.with_completion(callback(first.clone()));
    let second_frame = base.with_completion(callback(second.clone()));

    assert_eq!(first_frame.id(), base.id());
    assert_eq!(second_frame.id(), base.id());
    assert_ne!(
        first_frame.presentation_id(),
        second_frame.presentation_id()
    );
    first_frame.complete();
    drop(second_frame);

    assert_eq!(first.load(Ordering::SeqCst), 1);
    assert_eq!(second.load(Ordering::SeqCst), 1);
}
