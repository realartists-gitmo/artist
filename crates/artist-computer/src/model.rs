//! The vocabulary every rung shares.
//!
//! The single most important property here is that [`Node::bounds`] never
//! reaches the model. Coordinates are the harness's business: the model names
//! things by anchor, and a name that no longer resolves is an error rather than
//! a click on whatever happens to occupy that position now.

use serde::{Deserialize, Serialize};

/// How far up the abstraction ladder a surface is driven.
///
/// Lower is better, and the difference is not marginal: pausing a media player
/// over its D-Bus interface is one call, while clicking its pause button is a
/// screenshot, a tree walk, a coordinate resolution and a synthetic input event
/// that can all fail independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rung {
    /// The application's own API: D-Bus, a CLI, a localhost endpoint.
    Programmatic = 0,
    /// An engine protocol that already models the content: CDP, a PTY screen.
    Engine = 1,
    /// The platform accessibility tree.
    Accessibility = 2,
    /// Pixels. Observation only.
    Pixels = 3,
}

impl Rung {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// How the rung is named in an error the model reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Programmatic => "programmatic",
            Self::Engine => "engine",
            Self::Accessibility => "accessibility",
            Self::Pixels => "pixel",
        }
    }
}

/// A surface-local, stable identity for one element.
///
/// Opaque on purpose: a CDP backend node id, an AT-SPI object path, a PTY row
/// and an adapter-declared action all flatten to the same type, which is what
/// lets one anchor implementation serve every rung.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Binding(pub String);

impl Binding {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SurfaceId(pub String);

impl SurfaceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SurfaceId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A normalized element role.
///
/// Small and closed apart from `Other`, because the model reasons better about
/// a handful of familiar words than about each toolkit's native vocabulary
/// (`AXButton`, `push button`, `role=button` are all `Button` here).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Button,
    Link,
    TextBox,
    CheckBox,
    RadioButton,
    ComboBox,
    ListItem,
    MenuItem,
    Tab,
    Heading,
    Text,
    Image,
    Row,
    Cursor,
    Window,
    Other(String),
}

impl Role {
    pub fn label(&self) -> &str {
        match self {
            Self::Button => "button",
            Self::Link => "link",
            Self::TextBox => "textbox",
            Self::CheckBox => "checkbox",
            Self::RadioButton => "radio",
            Self::ComboBox => "combobox",
            Self::ListItem => "item",
            Self::MenuItem => "menuitem",
            Self::Tab => "tab",
            Self::Heading => "heading",
            Self::Text => "text",
            Self::Image => "image",
            Self::Row => "row",
            Self::Cursor => "cursor",
            Self::Window => "window",
            Self::Other(name) => name,
        }
    }

    /// Whether an element of this role is worth always showing.
    ///
    /// Interactive nodes survive the render budget unconditionally; static text
    /// is what gets coalesced and truncated when a surface is large.
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            Self::Button
                | Self::Link
                | Self::TextBox
                | Self::CheckBox
                | Self::RadioButton
                | Self::ComboBox
                | Self::MenuItem
                | Self::Tab
                | Self::ListItem
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeState {
    pub focused: bool,
    pub disabled: bool,
    pub checked: bool,
    pub expanded: bool,
    pub selected: bool,
    pub offscreen: bool,
}

impl NodeState {
    /// Compact flags for the rendered line; empty when nothing is notable.
    pub fn flags(&self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        for (set, name) in [
            (self.focused, "focused"),
            (self.disabled, "disabled"),
            (self.checked, "checked"),
            (self.expanded, "expanded"),
            (self.selected, "selected"),
            (self.offscreen, "offscreen"),
        ] {
            if set {
                flags.push(name);
            }
        }
        flags
    }
}

/// One element of a surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub binding: Binding,
    pub role: Role,
    /// The accessible name — what a person would call this thing.
    pub name: String,
    pub value: Option<String>,
    pub state: NodeState,
    /// Backend-specific action names this node supports.
    pub actions: Vec<String>,
    /// Harness-side only. Never rendered; see the module docs.
    pub bounds: Option<Rect>,
    pub depth: u16,
}

impl Node {
    pub fn new(binding: impl Into<String>, role: Role, name: impl Into<String>) -> Self {
        Self {
            binding: Binding::new(binding),
            role,
            name: name.into(),
            value: None,
            state: NodeState::default(),
            actions: Vec::new(),
            bounds: None,
            depth: 0,
        }
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn with_depth(mut self, depth: u16) -> Self {
        self.depth = depth;
        self
    }

    pub fn with_actions<I, S>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.actions = actions.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_bounds(mut self, bounds: Rect) -> Self {
        self.bounds = Some(bounds);
        self
    }

    pub fn with_state(mut self, state: NodeState) -> Self {
        self.state = state;
        self
    }

    /// The content hash used to detect a change between epochs.
    ///
    /// Covers everything the model is shown and nothing it is not — notably not
    /// `bounds`, so a surface that merely reflows produces no spurious deltas.
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.role.label().as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.name.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.value.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"\x1f");
        for flag in self.state.flags() {
            hasher.update(flag.as_bytes());
            hasher.update(b",");
        }
        // Actions are part of what the model is shown and part of what it can
        // do: an element that stops offering `close` has changed in the only way
        // that matters, and leaving it out meant no delta line said so.
        hasher.update(b"\x1f");
        for action in &self.actions {
            hasher.update(action.as_bytes());
            hasher.update(b",");
        }
        *hasher.finalize().as_bytes()
    }
}

/// What a surface can actually do, so the tool can refuse rather than pretend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub click: bool,
    pub type_text: bool,
    pub key: bool,
    pub scroll: bool,
    pub pixels: bool,
}

/// A captured frame.
///
/// Holds decoded RGBA rather than an encoded image so that set-of-mark
/// annotation can draw into it; [`Frame::to_png`] produces what actually
/// reaches the attachment store and the model.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
        }
    }

    /// The pixel at `(x, y)` as `(r, g, b, a)`, or `None` when out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = ((y * self.width + x) * 4) as usize;
        let pixel = self.rgba.get(offset..offset + 4)?;
        Some((pixel[0], pixel[1], pixel[2], pixel[3]))
    }

    /// Decode a PNG into RGBA.
    ///
    /// Backends differ in what they can hand over — the compositor reads raw
    /// pixels out of its own buffer, CDP returns an encoded screenshot — and
    /// the ladder only works if a frame means the same thing at every rung.
    /// Normalizing here costs one decode and is what lets the set-of-mark
    /// overlay draw on any surface's picture without knowing where it came from.
    pub fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|error| format!("read png: {error}"))?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| "png is too large to decode".to_owned())?;
        let mut buffer = vec![0u8; size];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|error| format!("decode png: {error}"))?;
        buffer.truncate(info.buffer_size());

        // Screenshots come back as RGB about as often as RGBA, and a viewer
        // handed three-byte pixels as though they were four sees a sheared
        // rainbow rather than a picture.
        let rgba = match info.color_type {
            png::ColorType::Rgba => buffer,
            png::ColorType::Rgb => buffer
                .chunks_exact(3)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
                .collect(),
            other => return Err(format!("unsupported png colour type {other:?}")),
        };
        Ok(Self::new(info.width, info.height, rgba))
    }

    /// Encode to PNG for the attachment store.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|error| format!("png header: {error}"))?;
            writer
                .write_image_data(&self.rgba)
                .map_err(|error| format!("png data: {error}"))?;
        }
        Ok(out)
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

/// The full current element set of a surface.
///
/// Always complete: diffing is [`AnchorBook`](crate::anchors::AnchorBook)'s job,
/// so every rung inherits identical delta semantics rather than each backend
/// inventing its own idea of what changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub nodes: Vec<Node>,
}

impl Snapshot {
    pub fn new(nodes: Vec<Node>) -> Self {
        Self { nodes }
    }
}
