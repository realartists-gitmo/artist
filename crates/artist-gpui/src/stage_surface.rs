use artist_session_host::StageDescriptor;
use gpui::{DmaBufPlane, DmaBufSurface};
use std::{collections::HashMap, os::fd::OwnedFd, sync::Arc};

/// Imported stage buffers are keyed for the lifetime of a host export. Frame
/// presents select an already-imported image and attach a one-shot completion
/// callback that releases the compositor buffer only after the displayed frame
/// and every in-flight renderer reference have been dropped.
#[derive(Default)]
pub(crate) struct StageSurfaces {
    surfaces: HashMap<(String, String, String), StageSurface>,
}

pub(crate) struct StageSurface {
    buffers: HashMap<u32, DmaBufSurface>,
    pub width: u32,
    pub height: u32,
    pub pending: Option<PresentedFrame>,
}

#[derive(Clone)]
pub(crate) struct PresentedFrame {
    pub surface: DmaBufSurface,
}

impl StageSurfaces {
    pub fn export(
        &mut self,
        root_session: String,
        lineage: String,
        stage: String,
        descriptor: StageDescriptor,
        descriptors: Vec<OwnedFd>,
    ) -> Result<(), String> {
        validate_export(&descriptor, descriptors.len())?;
        let planes = descriptors
            .into_iter()
            .zip(descriptor.offsets.iter().copied())
            .zip(descriptor.strides.iter().copied())
            .map(|((fd, offset), stride)| DmaBufPlane::new(fd, offset, stride))
            .collect();
        let image = DmaBufSurface::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
            descriptor.modifier,
            planes,
            None,
        );
        let surface = self
            .surfaces
            .entry((root_session, lineage, stage))
            .or_insert_with(|| StageSurface {
                buffers: HashMap::new(),
                width: descriptor.width,
                height: descriptor.height,
                pending: None,
            });
        if surface.width != descriptor.width || surface.height != descriptor.height {
            surface.buffers.clear();
            surface.pending = None;
            surface.width = descriptor.width;
            surface.height = descriptor.height;
        }
        surface.buffers.insert(descriptor.buffer_index, image);
        Ok(())
    }

    pub fn present(
        &mut self,
        root_session: &str,
        lineage: &str,
        stage: &str,
        buffer_index: u32,
        damage: Vec<[u32; 4]>,
        completion: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        let surface = self
            .surfaces
            .get_mut(&(
                root_session.to_owned(),
                lineage.to_owned(),
                stage.to_owned(),
            ))
            .ok_or_else(|| "stage presented before exporting buffers".to_owned())?;
        let image = surface
            .buffers
            .get(&buffer_index)
            .ok_or_else(|| format!("stage presented unknown buffer {buffer_index}"))?
            .with_completion(completion);
        let _damage = damage;
        surface.pending = Some(PresentedFrame { surface: image });
        Ok(())
    }

    pub fn current(&self, root_session: &str, lineage: &str, stage: &str) -> Option<DmaBufSurface> {
        self.surfaces
            .get(&(
                root_session.to_owned(),
                lineage.to_owned(),
                stage.to_owned(),
            ))?
            .pending
            .as_ref()
            .map(|frame| frame.surface.clone())
    }
}

fn validate_export(descriptor: &StageDescriptor, received_fds: usize) -> Result<(), String> {
    if descriptor.width == 0 || descriptor.height == 0 {
        return Err("stage buffer has an empty extent".into());
    }
    if descriptor.fd_count as usize != received_fds {
        return Err(format!(
            "stage export declared {} descriptors but received {received_fds}",
            descriptor.fd_count
        ));
    }
    if received_fds == 0
        || descriptor.strides.len() != received_fds
        || descriptor.offsets.len() != received_fds
    {
        return Err("stage export plane metadata does not match its descriptors".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        os::fd::OwnedFd,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    fn descriptor(index: u32) -> StageDescriptor {
        StageDescriptor {
            buffer_index: index,
            width: 1280,
            height: 720,
            format: 875_713_112,
            modifier: 0,
            strides: vec![5120],
            offsets: vec![0],
            fd_count: 1,
        }
    }

    fn fd() -> OwnedFd {
        File::open("/dev/null").unwrap().into()
    }

    #[test]
    fn descriptors_are_imported_once_and_presentations_get_fresh_completion_tokens() {
        let mut stages = StageSurfaces::default();
        stages
            .export(
                "root".into(),
                "main".into(),
                "desktop".into(),
                descriptor(2),
                vec![fd()],
            )
            .unwrap();
        let completed = Arc::new(AtomicBool::new(false));
        let signal = completed.clone();
        stages
            .present(
                "root",
                "main",
                "desktop",
                2,
                vec![[4, 5, 20, 30]],
                Arc::new(move || signal.store(true, Ordering::Release)),
            )
            .unwrap();
        let frame = stages.current("root", "main", "desktop").unwrap();
        assert_eq!(
            frame.id(),
            stages.current("root", "main", "desktop").unwrap().id()
        );
        frame.complete();
        assert!(completed.load(Ordering::Acquire));

        let second = Arc::new(AtomicBool::new(false));
        let signal = second.clone();
        stages
            .present(
                "root",
                "main",
                "desktop",
                2,
                vec![],
                Arc::new(move || signal.store(true, Ordering::Release)),
            )
            .unwrap();
        stages
            .current("root", "main", "desktop")
            .unwrap()
            .complete();
        assert!(second.load(Ordering::Acquire));
    }

    #[test]
    fn replacement_waits_for_in_flight_renderer_references_before_release() {
        let mut stages = StageSurfaces::default();
        for index in [1, 2] {
            stages
                .export(
                    "root".into(),
                    "main".into(),
                    "desktop".into(),
                    descriptor(index),
                    vec![fd()],
                )
                .unwrap();
        }

        let first_released = Arc::new(AtomicBool::new(false));
        let signal = first_released.clone();
        stages
            .present(
                "root",
                "main",
                "desktop",
                1,
                vec![],
                Arc::new(move || signal.store(true, Ordering::Release)),
            )
            .unwrap();
        let in_flight = stages.current("root", "main", "desktop").unwrap();

        stages
            .present("root", "main", "desktop", 2, vec![], Arc::new(|| {}))
            .unwrap();
        assert!(!first_released.load(Ordering::Acquire));

        drop(in_flight);
        assert!(first_released.load(Ordering::Acquire));
    }
    #[test]
    fn root_sessions_do_not_collide() {
        let mut stages = StageSurfaces::default();
        for root in ["one", "two"] {
            stages
                .export(
                    root.into(),
                    "main".into(),
                    "desktop".into(),
                    descriptor(0),
                    vec![fd()],
                )
                .unwrap();
        }
        assert!(stages.current("one", "main", "desktop").is_none());
        assert!(stages.current("two", "main", "desktop").is_none());
    }

    #[test]
    fn rejects_missing_descriptor_planes() {
        let mut stages = StageSurfaces::default();
        assert!(
            stages
                .export(
                    "root".into(),
                    "main".into(),
                    "desktop".into(),
                    descriptor(0),
                    vec![],
                )
                .is_err()
        );
    }
}
