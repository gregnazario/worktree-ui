use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Render, SharedString, Styled, Window, div, px, rgb,
};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// Minimal one-line text input built on raw key events. ASCII-oriented: no
/// IME/marked-text support in v1. Enter/escape are left to parent containers
/// (key events bubble up the dispatch tree).
pub struct TextField {
    pub value: String,
    cursor: usize,
    placeholder: SharedString,
    pub focus_handle: FocusHandle,
    id: usize,
}

impl TextField {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            placeholder: SharedString::from(placeholder.to_string()),
            focus_handle: cx.focus_handle(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn set_value(&mut self, v: &str, cx: &mut Context<Self>) {
        self.cursor = self.cursor.min(v.len());
        self.value = v.to_string();
        cx.notify();
    }

    fn insert(&mut self, ch: char, cx: &mut Context<Self>) {
        self.value.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        cx.notify();
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            let prev = prev_char_boundary(&self.value, self.cursor);
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
}

fn prev_char_boundary(s: &str, cursor: usize) -> usize {
    s[..cursor]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn next_char_boundary(s: &str, cursor: usize) -> usize {
    s[cursor..]
        .char_indices()
        .nth(1)
        .map(|(i, _)| cursor + i)
        .unwrap_or(s.len())
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let mut text = String::with_capacity(self.value.len() + 1);
        text.push_str(&self.value[..self.cursor]);
        if focused {
            text.push('|');
        }
        text.push_str(&self.value[self.cursor..]);

        let (border_color, bg) = if focused {
            (rgb(0x89b4fa), rgb(0x313244))
        } else {
            (rgb(0x45475a), rgb(0x181825))
        };

        let mut field = div()
            .id(self.id)
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .min_w(px(160.))
            .h(px(26.))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(border_color)
            .bg(bg)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _cx| {
                    window.focus(&this.focus_handle);
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let ks = &event.keystroke;
                if ks.modifiers.control || ks.modifiers.platform {
                    return;
                }
                match ks.key.as_str() {
                    "backspace" => this.backspace(cx),
                    "delete" => this.delete_forward(cx),
                    "left" => {
                        if this.cursor > 0 {
                            this.cursor = prev_char_boundary(&this.value, this.cursor);
                            cx.notify();
                        }
                    }
                    "right" => {
                        if this.cursor < this.value.len() {
                            this.cursor = next_char_boundary(&this.value, this.cursor);
                            cx.notify();
                        }
                    }
                    "home" => {
                        this.cursor = 0;
                        cx.notify();
                    }
                    "end" => {
                        this.cursor = this.value.len();
                        cx.notify();
                    }
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

        if self.value.is_empty() {
            field = field.text_color(rgb(0x6c7086)).child(self.placeholder.clone());
        } else {
            field = field.text_color(rgb(0xcdd6f4)).child(text);
        }
        field
    }
}
