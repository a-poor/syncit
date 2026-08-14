mod text_input;

use gpui::{
    App, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyBinding, Window,
    WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};

use crate::text_input::TextInput;

actions!(syncit, [Quit]);

struct SyncitApp {
    focus_handle: FocusHandle,
    name: Entity<TextInput>,
    desc: Entity<TextInput>,
    active: bool,
    count: i64,
}

impl SyncitApp {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            name: cx.new(|cx| TextInput::new(cx, "Name…", false)),
            desc: cx.new(|cx| TextInput::new(cx, "Description…", true)),
            active: false,
            count: 0,
        }
    }

    fn toggle_active(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.active = !self.active;
        cx.notify();
    }

    fn row(label: &'static str, content: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .items_start()
            .gap_3()
            .child(div().w(px(64.)).py_1().text_color(rgb(0xa6adc8)).child(label))
            .child(div().flex_1().child(content))
    }

    fn counter_button(
        &self,
        id: &'static str,
        label: &'static str,
        delta: i64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .w(px(28.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(0x45475a))
            .hover(|style| style.bg(rgb(0x585b70)))
            .cursor_pointer()
            .child(label)
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.count += delta;
                cx.notify();
            }))
    }
}

impl Focusable for SyncitApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SyncitApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let checkbox = div()
            .id("active-checkbox")
            .w(px(22.))
            .h(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .border_1()
            .border_color(rgb(0x585b70))
            .bg(if self.active {
                rgb(0x89b4fa)
            } else {
                rgb(0x313244)
            })
            .cursor_pointer()
            .when(self.active, |this| {
                this.text_color(rgb(0x1e1e2e)).child("✓")
            })
            .on_click(cx.listener(Self::toggle_active));

        let counter = div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.counter_button("count-dec", "-", -1, cx))
            .child(
                div()
                    .w(px(48.))
                    .flex()
                    .justify_center()
                    .child(self.count.to_string()),
            )
            .child(self.counter_button("count-inc", "+", 1, cx));

        div()
            .flex()
            .flex_col()
            .gap_3()
            .size_full()
            .p_4()
            .track_focus(&self.focus_handle(cx))
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child(Self::row("Name", self.name.clone()))
            .child(Self::row("Active?", checkbox))
            .child(Self::row("Count", counter))
            .child(Self::row("Desc", self.desc.clone()))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        text_input::bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);

        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_window, cx| cx.new(SyncitApp::new),
            )
            .unwrap();

        window
            .update(cx, |app, window, cx| {
                window.focus(&app.name.focus_handle(cx), cx);
            })
            .unwrap();
        cx.activate(true);
    });
}
