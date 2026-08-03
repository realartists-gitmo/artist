use artist_session_host::StageDescriptor;
use std::{collections::HashMap, os::fd::OwnedFd};

/// Imported stage buffers are keyed for the lifetime of a host export. Frame
/// presents only select a buffer and update damaged regions; they never resend
/// descriptors or pixel data.
#[derive(Default)]
pub(crate) struct StageSurfaces {
    surfaces: HashMap<(String, String), StageSurface>,
}

pub(crate) struct StageSurface {
    buffers: HashMap<u32, ExportedBuffer>,
    pub width: u32,
    pub height: u32,
    pub pending: Option<PresentedFrame>,
}

pub(crate) struct ExportedBuffer {
    pub descriptor: StageDescriptor,
    pub descriptors: Vec<OwnedFd>,
    pub submitted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PresentedFrame {
    pub buffer_index: u32,
    pub damage: Vec<[u32; 4]>,
}

impl StageSurfaces {
    pub fn export(
        &mut self,
        lineage: String,
        stage: String,
        descriptor: StageDescriptor,
        descriptors: Vec<OwnedFd>,
    ) -> Result<(), String> {
        validate_export(&descriptor, descriptors.len())?;
        let surface = self
            .surfaces
            .entry((lineage, stage))
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
        surface.buffers.insert(
            descriptor.buffer_index,
            ExportedBuffer {
                descriptor,
                descriptors,
                submitted: false,
            },
        );
        Ok(())
    }

    pub fn present(
        &mut self,
        lineage: &str,
        stage: &str,
        buffer_index: u32,
        damage: Vec<[u32; 4]>,
    ) -> Result<(), String> {
        let surface = self
            .surfaces
            .get_mut(&(lineage.to_owned(), stage.to_owned()))
            .ok_or_else(|| "stage presented before exporting buffers".to_owned())?;
        let buffer = surface
            .buffers
            .get_mut(&buffer_index)
            .ok_or_else(|| format!("stage presented unknown buffer {buffer_index}"))?;
        if buffer.submitted {
            return Err(format!("stage reused in-flight buffer {buffer_index}"));
        }
        let bounds = [0, 0, surface.width, surface.height];
        surface.pending = Some(PresentedFrame {
            buffer_index,
            damage: if damage.is_empty() {
                vec![bounds]
            } else {
                damage
            },
        });
        Ok(())
    }

    pub fn mark_submitted(&mut self, lineage: &str, stage: &str) -> Option<u32> {
        let surface = self
            .surfaces
            .get_mut(&(lineage.to_owned(), stage.to_owned()))?;
        let frame = surface.pending.take()?;
        surface.buffers.get_mut(&frame.buffer_index)?.submitted = true;
        Some(frame.buffer_index)
    }

    /// Called only from the renderer's submission-completion callback. The
    /// returned index is then safe to release to the compositor through the
    /// session host.
    pub fn submission_complete(
        &mut self,
        lineage: &str,
        stage: &str,
        buffer_index: u32,
    ) -> Option<u32> {
        let buffer = self
            .surfaces
            .get_mut(&(lineage.to_owned(), stage.to_owned()))?
            .buffers
            .get_mut(&buffer_index)?;
        if !buffer.submitted {
            return None;
        }
        buffer.submitted = false;
        Some(buffer_index)
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
    use std::{fs::File, os::fd::OwnedFd};

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
    fn descriptors_are_imported_once_and_released_after_submission() {
        let mut stages = StageSurfaces::default();
        stages
            .export("main".into(), "desktop".into(), descriptor(2), vec![fd()])
            .unwrap();
        stages
            .present("main", "desktop", 2, vec![[4, 5, 20, 30]])
            .unwrap();
        assert_eq!(stages.mark_submitted("main", "desktop"), Some(2));
        assert!(stages.present("main", "desktop", 2, vec![]).is_err());
        assert_eq!(stages.submission_complete("main", "desktop", 2), Some(2));
        stages.present("main", "desktop", 2, vec![]).unwrap();
    }

    #[test]
    fn rejects_missing_descriptor_planes() {
        let mut stages = StageSurfaces::default();
        assert!(
            stages
                .export("main".into(), "desktop".into(), descriptor(0), vec![])
                .is_err()
        );
    }
}
