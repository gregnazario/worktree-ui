//! Minimal multi-line text input built on raw key events — the same
//! primitives as `text_field.rs`, extended across lines. ASCII-oriented:
//! no IME/marked-text support in v1 (standing GPUI 0.2.2 limitation).
//! No wrapping: long lines scroll horizontally in their row.
//!
//! Enter/escape bubble to parent containers EXCEPT that enter inserts a
//! newline here first — a multi-line editor owns enter. Confirm keys
//! (cmd/ctrl+enter) are handled by the parent card, which sees them
//! because this handler ignores platform/control-modified keystrokes.

use gpui::{
    div, px, rgb, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Render, Styled, Window,
};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

pub struct TextArea {
    /// Full text including `\n` separators.
    value: String,
    /// Byte offset into `value`, always on a char boundary.
    cursor: usize,
    pub focus_handle: FocusHandle,
    id: usize,
}

impl TextArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            focus_handle: cx.focus_handle(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn set_value(&mut self, v: &str, cx: &mut Context<Self>) {
        self.value = v.to_string();
        self.cursor = self.cursor.min(self.value.len());
        self.snap_to_char_boundary();
        cx.notify();
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// (line index, char column within the line) for cursor movement.
    fn line_col(&self) -> (usize, usize) {
        let before = &self.value[..self.cursor];
        let line = before.matches('\n').count();
        let col = before.rsplit('\n').next().unwrap_or(before).chars().count();
        (line, col)
    }

    fn lines(&self) -> Vec<&str> {
        self.value.split('\n').collect()
    }

    /// Byte offset of the start of `line`.
    fn line_start(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        // The `line`th newline ENDS line `line - 1`; its byte after is
        // the start of `line`.
        self.value
            .match_indices('\n')
            .nth(line - 1)
            .map(|(i, _)| i + 1)
            .unwrap_or(0)
    }

    fn snap_to_char_boundary(&mut self) {
        while self.cursor > 0 && !self.value.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
    }

    fn insert(&mut self, ch: char, cx: &mut Context<Self>) {
        self.value.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        cx.notify();
    }

    fn newline(&mut self, cx: &mut Context<Self>) {
        self.insert('\n', cx);
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            let prev = self.value[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.value.drain(prev..self.cursor);
            self.cursor = prev;
            cx.notify();
        }
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.cursor < self.value.len() {
            let next = self.value[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.value.len());
            self.value.drain(self.cursor..next);
            cx.notify();
        }
    }

    fn move_left(&mut self, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            self.cursor = self.value[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            cx.notify();
        }
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        if self.cursor < self.value.len() {
            self.cursor = self.value[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.value.len());
            cx.notify();
        }
    }

    fn move_up(&mut self, cx: &mut Context<Self>) {
        let (line, col) = self.line_col();
        if line == 0 {
            return;
        }
        let target_start = self.line_start(line - 1);
        let target_line = self.value[target_start..]
            .split('\n')
            .next()
            .unwrap_or_default();
        let offset: usize = target_line.chars().take(col).map(char::len_utf8).sum();
        self.cursor = target_start + offset;
        cx.notify();
    }

    fn move_down(&mut self, cx: &mut Context<Self>) {
        let (line, col) = self.line_col();
        let lines = self.lines();
        if line + 1 >= lines.len() {
            return;
        }
        let target_start = self.line_start(line + 1);
        let target_line = lines[line + 1];
        let offset: usize = target_line.chars().take(col).map(char::len_utf8).sum();
        self.cursor = target_start + offset;
        cx.notify();
    }

    fn move_home(&mut self, cx: &mut Context<Self>) {
        let (line, _) = self.line_col();
        self.cursor = self.line_start(line);
        cx.notify();
    }

    fn move_end(&mut self, cx: &mut Context<Self>) {
        let (line, _) = self.line_col();
        let lines = self.lines();
        self.cursor = match lines.get(line) {
            Some(l) if line + 1 < lines.len() => self.line_start(line) + l.len(),
            _ => self.value.len(),
        };
        cx.notify();
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let mut wrap = div()
            .id(self.id)
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .w_full()
            .h(px(220.))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(if focused {
                rgb(0x89b4fa)
            } else {
                rgb(0x45475a)
            })
            .bg(rgb(0x181825))
            .p_2()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _cx| {
                    window.focus(&this.focus_handle);
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let ks = &event.keystroke;
                // Paste is consumed here even though other cmd/ctrl
                // chords belong to the parent card (cmd+enter commits).
                if (ks.modifiers.platform || ks.modifiers.control)
                    && !ks.modifiers.alt
                    && ks.key == "v"
                {
                    cx.stop_propagation();
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        // Normalize foreign line endings; insert as typed.
                        for ch in text.replace("\r\n", "\n").chars() {
                            this.insert(ch, cx);
                        }
                    }
                    return;
                }
                // Remaining cmd/ctrl chords belong to the parent card
                // (cmd+enter commits): let them bubble. Keys the area
                // CONSUMES must not also reach the ancestor handlers —
                // but keys it ignores (escape: the card cancels) must.
                if ks.modifiers.control || ks.modifiers.platform || ks.modifiers.alt {
                    return;
                }
                match ks.key.as_str() {
                    "enter" | "backspace" | "delete" | "left" | "right" | "up" | "down"
                    | "home" | "end" | "tab" | "space" => {}
                    k if k.chars().count() == 1 => {}
                    _ => return,
                }
                cx.stop_propagation();
                match ks.key.as_str() {
                    "enter" => this.newline(cx),
                    "backspace" => this.backspace(cx),
                    "delete" => this.delete_forward(cx),
                    "left" => this.move_left(cx),
                    "right" => this.move_right(cx),
                    "up" => this.move_up(cx),
                    "down" => this.move_down(cx),
                    "home" => this.move_home(cx),
                    "end" => this.move_end(cx),
                    "tab" => this.insert('\t', cx),
                    "space" => this.insert(' ', cx),
                    key if key.chars().count() == 1 => {
                        let typed = ks.key_char.clone().unwrap_or_else(|| key.to_string());
                        if let Some(ch) = typed.chars().next() {
                            this.insert(ch, cx);
                        }
                    }
                    _ => {}
                }
            }));

        // Vertical follow: a 220px box shows ~12 rows at the default
        // line height. Keep the cursor line inside a window of lines
        // (the commit case is a short subject; this bounds the v1 gap
        // for long drafts without a full scroll-offset model).
        const VISIBLE_LINES: usize = 12;
        let all_lines = self.lines();
        let (cursor_line, _) = self.line_col();
        let scroll_line = if cursor_line >= VISIBLE_LINES {
            cursor_line + 1 - VISIBLE_LINES
        } else {
            0
        };

        for (i, line) in all_lines
            .into_iter()
            .enumerate()
            .skip(scroll_line)
            .take(VISIBLE_LINES.max(cursor_line + 1 - scroll_line))
        {
            let mut text = String::with_capacity(line.len() + 1);
            text.push_str(line);
            if focused && i == cursor_line {
                // The cursor byte offset may sit mid-line; render the
                // bar by splicing at the offset instead of the end.
                let line_start = self.line_start(i);
                let off = self.cursor.saturating_sub(line_start).min(line.len());
                text.clear();
                text.push_str(&line[..off]);
                text.push('|');
                text.push_str(&line[off..]);
            }
            // Preserve blank lines: an empty row still needs height.
            let rendered: String = if text.is_empty() {
                " ".to_string()
            } else {
                text
            };
            wrap = wrap.child(div().flex().text_color(rgb(0xcdd6f4)).child(rendered));
        }
        wrap
    }
}
