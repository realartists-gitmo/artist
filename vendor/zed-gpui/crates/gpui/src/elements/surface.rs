use crate::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    ObjectFit, Pixels, Style, StyleRefinement, Styled, Window,
};
#[cfg(target_os = "macos")]
use core_video::pixel_buffer::CVPixelBuffer;
use refineable::Refineable;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::{
    fmt,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
static NEXT_DMA_BUF_IMAGE_ID: AtomicU64 = AtomicU64::new(1);
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
static NEXT_DMA_BUF_PRESENTATION_ID: AtomicU64 = AtomicU64::new(1);
/// One plane in a Linux DMA-BUF surface.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub struct DmaBufPlane {
    fd: OwnedFd,
    offset: u32,
    stride: u32,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl DmaBufPlane {
    /// Construct a DMA-BUF plane from an owned descriptor and explicit layout.
    pub fn new(fd: OwnedFd, offset: u32, stride: u32) -> Self {
        Self { fd, offset, stride }
    }

    /// Borrow the plane descriptor.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Byte offset of the plane in the allocation.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// Bytes between adjacent rows.
    pub fn stride(&self) -> u32 {
        self.stride
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
struct DmaBufImageInner {
    id: u64,
    width: u32,
    height: u32,
    format: u32,
    modifier: u64,
    planes: Vec<DmaBufPlane>,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
struct DmaBufCompletion {
    callback: Arc<dyn Fn() + Send + Sync>,
    id: u64,
    completed: AtomicBool,
}
impl DmaBufCompletion {
    fn complete(&self) {
        if !self.completed.swap(true, Ordering::AcqRel) {
            (self.callback)();
        }
    }
}

impl Drop for DmaBufCompletion {
    fn drop(&mut self) {
        self.complete();
    }
}

/// A zero-copy Linux surface backed by DMA-BUF file descriptors.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[derive(Clone)]
pub struct DmaBufSurface {
    image: Arc<DmaBufImageInner>,
    completion: Option<Arc<DmaBufCompletion>>,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl DmaBufSurface {
    /// Construct a surface. Descriptors remain owned until the renderer imports
    /// them. The completion callback runs once when the displayed frame and all
    /// in-flight renderer references have been dropped, or when `complete` is
    /// called explicitly after an unrecoverable import failure.
    pub fn new(
        width: u32,
        height: u32,
        format: u32,
        modifier: u64,
        planes: Vec<DmaBufPlane>,
        completion: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self {
            image: Arc::new(DmaBufImageInner {
                id: NEXT_DMA_BUF_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
                width,
                height,
                format,
                modifier,
                planes,
            }),
            completion: completion.map(|callback| {
                Arc::new(DmaBufCompletion {
                    callback,
                    id: NEXT_DMA_BUF_PRESENTATION_ID.fetch_add(1, Ordering::Relaxed),
                    completed: AtomicBool::new(false),
                })
            }),
        }
    }

    /// Attach a new one-shot completion callback while reusing the imported image.
    pub fn with_completion(&self, completion: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            image: self.image.clone(),
            completion: Some(Arc::new(DmaBufCompletion {
                callback: completion,
                id: NEXT_DMA_BUF_PRESENTATION_ID.fetch_add(1, Ordering::Relaxed),
                completed: AtomicBool::new(false),
            })),
        }
    }

    /// Identity of this one producer presentation. It changes for every
    /// `with_completion` call even when the imported image is reused.
    pub fn presentation_id(&self) -> Option<u64> {
        self.completion.as_ref().map(|completion| completion.id)
    }
    /// Stable identity used by the renderer's import cache.
    pub fn id(&self) -> u64 {
        self.image.id
    }
    /// Pixel width.
    pub fn width(&self) -> u32 {
        self.image.width
    }
    /// Pixel height.
    pub fn height(&self) -> u32 {
        self.image.height
    }
    /// DRM FourCC format code.
    pub fn format(&self) -> u32 {
        self.image.format
    }
    /// DRM format modifier.
    pub fn modifier(&self) -> u64 {
        self.image.modifier
    }
    /// Plane descriptors and layouts.
    pub fn planes(&self) -> &[DmaBufPlane] {
        &self.image.planes
    }

    /// Notify the producer immediately. Normal rendering relies on last-reference
    /// drop so a displayed frame stays leased across unrelated UI redraws.
    pub fn complete(&self) {
        if let Some(completion) = self.completion.as_ref() {
            completion.complete();
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl fmt::Debug for DmaBufSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DmaBufSurface")
            .field("id", &self.id())
            .field("width", &self.width())
            .field("height", &self.height())
            .field("format", &self.format())
            .field("modifier", &self.modifier())
            .field("planes", &self.planes().len())
            .finish()
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl PartialEq for DmaBufSurface {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl Eq for DmaBufSurface {}

/// A source of a surface's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceSource {
    /// A macOS image buffer from CoreVideo
    #[cfg(target_os = "macos")]
    Surface(CVPixelBuffer),
    /// A Linux DMA-BUF image.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    DmaBuf(DmaBufSurface),
}

#[cfg(target_os = "macos")]
impl From<CVPixelBuffer> for SurfaceSource {
    fn from(value: CVPixelBuffer) -> Self {
        SurfaceSource::Surface(value)
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl From<DmaBufSurface> for SurfaceSource {
    fn from(value: DmaBufSurface) -> Self {
        SurfaceSource::DmaBuf(value)
    }
}

/// A surface element.
pub struct Surface {
    source: SurfaceSource,
    object_fit: ObjectFit,
    style: StyleRefinement,
}

/// Create a new surface element.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "freebsd"))]
pub fn surface(source: impl Into<SurfaceSource>) -> Surface {
    Surface {
        source: source.into(),
        object_fit: ObjectFit::Contain,
        style: Default::default(),
    }
}

impl Surface {
    /// Set the object fit for the image.
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.object_fit = object_fit;
        self
    }
}

impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] window: &mut Window,
        _: &mut App,
    ) {
        match &self.source {
            #[cfg(target_os = "macos")]
            SurfaceSource::Surface(surface) => {
                let size = crate::size(surface.get_width().into(), surface.get_height().into());
                let new_bounds = self.object_fit.get_bounds(bounds, size);
                window.paint_surface(new_bounds, surface.clone());
            }
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            SurfaceSource::DmaBuf(surface) => {
                let size = crate::size(surface.width().into(), surface.height().into());
                let new_bounds = self.object_fit.get_bounds(bounds, size);
                window.paint_dma_buf_surface(new_bounds, surface.clone());
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

impl IntoElement for Surface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Surface {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod tests {
    use super::*;
    use std::{
        fs::File,
        sync::atomic::{AtomicUsize, Ordering},
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
        first_frame.complete();
        drop(second_frame);

        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 1);
    }
}
