#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TitleSegment {
    Prose(String),
    Input(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolTitle {
    pub segments: Vec<TitleSegment>,
}

impl ToolTitle {
    pub(crate) fn plain_text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| match segment {
                TitleSegment::Prose(text) | TitleSegment::Input(text) => text.as_str(),
            })
            .collect()
    }
}
