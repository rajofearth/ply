//! Hand-rolled single-line text field: state and input handling (spike A).
//!
//! Decision D2 in docs/research/size-frontier.md replaces the gpui-component
//! `Input` used by the filter and the rename row with this file, so the
//! component crate can leave the link. This spike builds the state half only:
//! the value, the selection, IME marked text, clipboard actions, and the
//! commit and cancel events. Nothing imports it yet, so it cannot change app
//! behavior. Painting (caret, selection, placeholder) and the caller swap
//! come next.
//!
//! The shape follows the gpui input example where it matters (UTF-16 index
//! conversion, marked-text handling, clipboard), and drops what a single-line
//! explorer field never uses: grapheme movement is char movement because
//! unicode-segmentation is not a direct dependency, and there is no
//! multiline, no validation, and no prefix or suffix slots.
//!
//! Swap map for the next spike:
//! - `InputState::new(window, cx)` becomes `FieldState::new(window, cx)`
//! - `InputEvent::{Change, PressEnter, Blur, Focus}` becomes `FieldEvent`
//! - `rename_event_action` becomes `field_event_action`
//! - `rename_select_range` becomes `stem_select_range`
//! - `input.base_state().update(cx, |b, cx| b.set_selected_range(r, cx))`
//!   becomes `field.update(cx, |f, cx| f.set_selected_range(r, cx))`

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    Pixels, ShapedLine, SharedString, Subscription, UTF16Selection, Window, actions, point,
};

/// Key context for the field. The swap spike binds keys under this context so
/// typing never leaks into the listing shortcuts.
pub const FIELD_KEY_CONTEXT: &str = "Field";

actions!(
    field,
    [
        FieldBackspace,
        FieldDelete,
        FieldSelectAll,
        FieldCut,
        FieldCopy,
        FieldPaste,
        FieldConfirm,
    ]
);

/// What a field can tell its owner. Same four variants the callers already
/// match on for the library input, so subscriptions move over unchanged.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldEvent {
    Change,
    PressEnter { secondary: bool, shift: bool },
    Focus,
    Blur,
}

/// What the owner should do with a [`FieldEvent`]. Pure, so the rename commit
/// path stays testable without a GPUI context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldAction {
    Commit,
    Cancel,
}

/// Enter commits, losing focus cancels, everything else means nothing. This is
/// `rename_event_action` with the library type swapped out.
pub fn field_event_action(event: &FieldEvent) -> Option<FieldAction> {
    match event {
        FieldEvent::PressEnter { .. } => Some(FieldAction::Commit),
        FieldEvent::Blur => Some(FieldAction::Cancel),
        FieldEvent::Change | FieldEvent::Focus => None,
    }
}

/// Byte range to pre-select when a rename edit opens: the stem up to the last
/// dot, so typing replaces the name but keeps the extension. Extensionless
/// names and dotfiles (a leading dot) select all. Pure: the dot is ASCII, so
/// the split is always a UTF-8 boundary and the range always stays ordered.
/// This is `rename_select_range` with the library swap factored out.
pub fn stem_select_range(name: &str) -> Range<usize> {
    if name.starts_with('.') {
        return 0..name.len();
    }
    match name.rfind('.') {
        Some(dot) => 0..dot,
        None => 0..name.len(),
    }
}

/// The testable half of the field: content, selection, and IME marked text.
/// Holds no handles and needs no context, so plain unit tests drive it.
/// [`FieldState`] owns one and adds focus, placeholder, and event plumbing.
#[derive(Clone, Debug, Default)]
pub struct FieldText {
    pub content: SharedString,
    pub selected_range: Range<usize>,
    pub selection_reversed: bool,
    pub marked_range: Option<Range<usize>>,
}

impl FieldText {
    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub fn move_to(&mut self, offset: usize) {
        self.selected_range = offset..offset;
    }

    pub fn select_to(&mut self, offset: usize) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
    }

    /// Previous char boundary. The gpui example walks graphemes; without a
    /// unicode-segmentation dependency this walks chars, which still keeps
    /// every offset on a UTF-8 boundary.
    pub fn previous_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        self.content[..offset]
            .chars()
            .next_back()
            .map(|ch| offset - ch.len_utf8())
            .unwrap_or(0)
    }

    /// Next char boundary. Same grapheme-to-char trade as
    /// [`FieldText::previous_boundary`].
    pub fn next_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        self.content[offset..]
            .chars()
            .next()
            .map(|ch| offset + ch.len_utf8())
            .unwrap_or(self.content.len())
    }

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    pub fn text_for_range(
        &self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    /// Splice `new_text` over the IME range, the marked range, or the
    /// selection, in that order. Collapses the selection to the end of the
    /// insert and clears the mark. The [`EntityInputHandler`] impl below calls
    /// this, then emits [`FieldEvent::Change`].
    pub fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
    }

    /// Composition update: same splice, but the new text stays marked and the
    /// selection follows the IME-supplied range. Empty text clears the mark.
    pub fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
    }
}

/// The entity the app will own: a [`FieldText`] plus focus, placeholder, and
/// the layout the paint half will need for IME bounds and mouse placement.
/// Construct with `cx.new(|cx| FieldState::new(window, cx))`, exactly like the
/// library state it replaces.
pub struct FieldState {
    focus_handle: FocusHandle,
    /// The text core. Read content and selection through here
    /// (`field.read(cx).text.content`) or through [`FieldState::value`] and
    /// [`FieldState::selected_range`].
    pub text: FieldText,
    pub placeholder: SharedString,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    subscriptions: Vec<Subscription>,
}

impl FieldState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let on_focus = cx.on_focus(&focus_handle, window, |_, _, cx| {
            cx.emit(FieldEvent::Focus);
        });
        let on_blur = cx.on_blur(&focus_handle, window, |_, _, cx| {
            cx.emit(FieldEvent::Blur);
        });
        Self {
            focus_handle,
            text: FieldText::default(),
            placeholder: SharedString::default(),
            last_layout: None,
            last_bounds: None,
            subscriptions: vec![on_focus, on_blur],
        }
    }

    pub fn value(&self) -> SharedString {
        self.text.content.clone()
    }

    /// Programmatic write. Like the library `set_value`, this moves the caret
    /// to the end and does not emit [`FieldEvent::Change`]; owners that clear
    /// the field this way already know the value changed.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.content = value.into();
        let len = self.text.content.len();
        self.text.selected_range = len..len;
        self.text.selection_reversed = false;
        self.text.marked_range = None;
        cx.notify();
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    /// Typing stand-down check for the app's `typing()`: true while this field
    /// holds keyboard focus, so bare-key shortcuts stay quiet.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub fn selected_range(&self) -> Range<usize> {
        self.text.selected_range.clone()
    }

    /// Clamp to the content and keep the range ordered, then repaint. This is
    /// what the rename stem pre-selection calls after `set_value`.
    pub fn set_selected_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let len = self.text.content.len();
        let (mut start, mut end) = (range.start.min(len), range.end.min(len));
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        self.text.selected_range = start..end;
        self.text.selection_reversed = false;
        cx.notify();
    }

    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.text.marked_range.clone()
    }

    fn replace_selection_with(&mut self, new_text: &str, cx: &mut Context<Self>) {
        self.text.replace_text_in_range(None, new_text);
        cx.emit(FieldEvent::Change);
        cx.notify();
    }

    fn backspace(&mut self, _: &FieldBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.text.selected_range.is_empty() {
            let prev = self.text.previous_boundary(self.text.cursor_offset());
            if self.text.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.text.select_to(prev);
        }
        self.replace_selection_with("", cx);
    }

    fn delete(&mut self, _: &FieldDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.text.selected_range.is_empty() {
            let next = self.text.next_boundary(self.text.cursor_offset());
            if self.text.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.text.select_to(next);
        }
        self.replace_selection_with("", cx);
    }

    fn select_all(&mut self, _: &FieldSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_to(0);
        self.text.select_to(self.text.content.len());
        cx.notify();
    }

    fn cut(&mut self, _: &FieldCut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.text.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text.content[self.text.selected_range.clone()].to_string(),
            ));
            self.replace_selection_with("", cx);
        }
    }

    fn copy(&mut self, _: &FieldCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.text.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text.content[self.text.selected_range.clone()].to_string(),
            ));
        }
    }

    fn paste(&mut self, _: &FieldPaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let single_line = text.replace('\n', " ");
            self.text.replace_text_in_range(None, &single_line);
            cx.emit(FieldEvent::Change);
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &FieldConfirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FieldEvent::PressEnter {
            secondary: false,
            shift: false,
        });
    }
}

impl EventEmitter<FieldEvent> for FieldState {}

impl Focusable for FieldState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for FieldState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        self.text.text_for_range(range_utf16, actual_range)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.text.range_to_utf16(&self.text.selected_range),
            reversed: self.text.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.text
            .marked_range
            .as_ref()
            .map(|range| self.text.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.text.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.replace_text_in_range(range_utf16, new_text);
        cx.emit(FieldEvent::Change);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.replace_and_mark_text_in_range(
            range_utf16,
            new_text,
            new_selected_range_utf16,
        );
        cx.emit(FieldEvent::Change);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // No painted layout yet (the paint half lands with the caller swap),
        // so there is nothing to anchor IME windows to.
        let line = self.last_layout.as_ref()?;
        let range = self.text.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                element_bounds.left() + line.x_for_index(range.start),
                element_bounds.top(),
            ),
            point(
                element_bounds.left() + line.x_for_index(range.end),
                element_bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let line = self.last_layout.as_ref()?;

        assert_eq!(line.text, self.text.content);
        let utf8_index = line.index_for_x(point.x - line_point.x)?;
        Some(self.text.offset_to_utf16(utf8_index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_with(content: &str, selected_range: Range<usize>) -> FieldText {
        FieldText {
            content: content.into(),
            selected_range,
            ..Default::default()
        }
    }

    #[test]
    fn event_map_matches_rename_contract() {
        assert_eq!(
            field_event_action(&FieldEvent::PressEnter {
                secondary: false,
                shift: false
            }),
            Some(FieldAction::Commit)
        );
        assert_eq!(
            field_event_action(&FieldEvent::PressEnter {
                secondary: true,
                shift: true
            }),
            Some(FieldAction::Commit)
        );
        assert_eq!(
            field_event_action(&FieldEvent::Blur),
            Some(FieldAction::Cancel)
        );
        assert_eq!(field_event_action(&FieldEvent::Change), None);
        assert_eq!(field_event_action(&FieldEvent::Focus), None);
    }

    #[test]
    fn stem_select_range_keeps_stem_first() {
        assert_eq!(stem_select_range("notes.txt"), 0..5);
        assert_eq!(stem_select_range("archive.tar.gz"), 0..11);
        assert_eq!(stem_select_range("Makefile"), 0.."Makefile".len());
        assert_eq!(stem_select_range(".gitignore"), 0..".gitignore".len());
        assert_eq!(stem_select_range(""), 0..0);
        assert_eq!(stem_select_range("café.txt"), 0.."café".len());
        for name in ["notes.txt", "archive.tar.gz", "Makefile", ".gitignore", "", "café.txt"] {
            let range = stem_select_range(name);
            assert!(
                range.start <= range.end,
                "selection must stay ordered for {name:?}"
            );
        }
        assert_eq!(&"notes.txt"[stem_select_range("notes.txt")], "notes");
    }

    #[test]
    fn replace_text_in_range_updates_content_and_selection() {
        // Typing over a selection replaces it and collapses the caret.
        let mut text = text_with("hello", 1..4);
        text.replace_text_in_range(None, "X");
        assert_eq!(text.content.to_string(), "hXo");
        assert_eq!(text.selected_range, 2..2);
        assert_eq!(text.marked_range, None);

        // Plain insert at the caret.
        let mut text = text_with("ho", 2..2);
        text.replace_text_in_range(None, "!");
        assert_eq!(text.content.to_string(), "ho!");
        assert_eq!(text.selected_range, 3..3);

        // Empty text deletes the selection.
        let mut text = text_with("hello", 1..4);
        text.replace_text_in_range(None, "");
        assert_eq!(text.content.to_string(), "ho");
        assert_eq!(text.selected_range, 1..1);

        // An explicit IME range wins over the selection.
        let mut text = text_with("hello", 0..0);
        text.replace_text_in_range(Some(1..4), "X");
        assert_eq!(text.content.to_string(), "hXo");
        assert_eq!(text.selected_range, 2..2);

        // A marked range wins when no explicit range arrives.
        let mut text = FieldText {
            marked_range: Some(1..4),
            ..text_with("hello", 0..0)
        };
        text.replace_text_in_range(None, "X");
        assert_eq!(text.content.to_string(), "hXo");
        assert_eq!(text.selected_range, 2..2);
        assert_eq!(text.marked_range, None);
    }

    #[test]
    fn replace_text_in_range_speaks_utf16() {
        // "a😀b": the emoji is one char, two UTF-16 units, four bytes.
        let mut text = text_with("a😀b", 0..0);
        text.replace_text_in_range(Some(1..3), "");
        assert_eq!(text.content.to_string(), "ab");
        assert_eq!(text.selected_range, 1..1);

        let text = text_with("a😀b", 1..5);
        assert_eq!(text.range_to_utf16(&text.selected_range), 1..3);
        let mut actual = None;
        assert_eq!(
            text.text_for_range(1..3, &mut actual),
            Some("😀".to_string())
        );
        assert_eq!(actual, Some(1..3));
    }

    #[test]
    fn replace_and_mark_keeps_composition_marked() {
        let mut text = text_with("hello", 5..5);
        text.replace_and_mark_text_in_range(None, "!", None);
        assert_eq!(text.content.to_string(), "hello!");
        assert_eq!(text.marked_range, Some(5..6));
        assert_eq!(text.selected_range, 6..6);

        // Committing empty text clears the mark.
        text.replace_and_mark_text_in_range(None, "", None);
        assert_eq!(text.marked_range, None);
    }

    #[test]
    fn char_boundaries_stay_on_valid_edges() {
        let text = text_with("café", 0..0);
        assert_eq!(text.previous_boundary(5), 3);
        assert_eq!(text.next_boundary(3), 5);
        assert_eq!(text.previous_boundary(0), 0);
        assert_eq!(text.next_boundary(5), 5);

        let text = text_with("a😀b", 0..0);
        assert_eq!(text.previous_boundary(5), 1);
        assert_eq!(text.next_boundary(1), 5);
    }
}
