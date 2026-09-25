use crate::TokenEvent;

/// Source position and trace shared while Code and Markup parsers alternate.
pub(super) struct ParseState<'a> {
    pub(super) source: &'a str,
    pub(super) documentation: bool,
    pub(super) pos: usize,
    pub(super) depth: usize,
    pub(super) tokens: Vec<TokenEvent>,
    pub(super) recovery: bool,
}

impl<'a> ParseState<'a> {
    pub(super) fn new(source: &'a str, documentation: bool) -> Self {
        Self {
            source,
            documentation,
            pos: 0,
            depth: 0,
            tokens: Vec::new(),
            recovery: false,
        }
    }

    pub(super) fn record(&mut self, start: usize, end: usize, kind: &'static str, text: &str) {
        let i = self.tokens.len();
        let text = if text.chars().count() > 32 {
            let mut cut: String = text.chars().take(31).collect();
            cut.push('…');
            cut
        } else {
            text.into()
        };
        self.tokens.push(TokenEvent {
            i,
            start,
            end,
            kind,
            text,
            recovery: self.recovery,
        });
    }

    pub(super) fn record_span(&mut self, start: usize, end: usize, kind: &'static str) {
        let source = self.source;
        self.record(start, end, kind, &source[start..end]);
    }
}
