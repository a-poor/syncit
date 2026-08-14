mod sync;
mod text_input;

use gpui::{
    App, AsyncApp, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyBinding, Task,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::sync::{LocalChange, SyncEvent, SyncStatus};
use crate::text_input::TextInput;

actions!(syncit, [Quit]);

struct SyncitApp {
    focus_handle: FocusHandle,
    name: Entity<TextInput>,
    desc: Entity<TextInput>,
    active: bool,
    count: i64,
    status: SyncStatus,
    changes: UnboundedSender<LocalChange>,
    // Last text values exchanged with the sync actor, used to tell genuine
    // local edits apart from remote updates we applied to the inputs ourselves.
    last_name: String,
    last_desc: String,
    _events_task: Task<()>,
}

impl SyncitApp {
    fn new(
        cx: &mut Context<Self>,
        changes: UnboundedSender<LocalChange>,
        mut events: UnboundedReceiver<SyncEvent>,
    ) -> Self {
        let name = cx.new(|cx| TextInput::new(cx, "Name…", false));
        let desc = cx.new(|cx| TextInput::new(cx, "Description…", true));

        cx.observe(&name, |this: &mut Self, input, cx| {
            let content = input.read(cx).content.to_string();
            if content != this.last_name {
                this.last_name = content.clone();
                this.changes.send(LocalChange::SetName(content)).ok();
            }
        })
        .detach();
        cx.observe(&desc, |this: &mut Self, input, cx| {
            let content = input.read(cx).content.to_string();
            if content != this.last_desc {
                this.last_desc = content.clone();
                this.changes.send(LocalChange::SetDesc(content)).ok();
            }
        })
        .detach();

        let events_task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            while let Some(event) = events.recv().await {
                let alive = this
                    .update(cx, |app, cx| app.on_sync_event(event, cx))
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            name,
            desc,
            active: false,
            count: 0,
            status: SyncStatus::Connecting,
            changes,
            last_name: String::new(),
            last_desc: String::new(),
            _events_task: events_task,
        }
    }

    fn on_sync_event(&mut self, event: SyncEvent, cx: &mut Context<Self>) {
        match event {
            SyncEvent::Status(status) => self.status = status,
            SyncEvent::Doc(doc) => {
                self.active = doc.active;
                self.count = doc.count.value();
                if doc.name != self.last_name {
                    self.last_name = doc.name.clone();
                    self.name
                        .update(cx, |input, cx| input.set_content(doc.name, cx));
                }
                let desc = doc.desc.as_str().to_string();
                if desc != self.last_desc {
                    self.last_desc = desc.clone();
                    self.desc.update(cx, |input, cx| input.set_content(desc, cx));
                }
            }
        }
        cx.notify();
    }

    fn toggle_active(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.active = !self.active;
        self.changes.send(LocalChange::SetActive(self.active)).ok();
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
                this.changes.send(LocalChange::IncrementCount(delta)).ok();
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

        let (dot_color, status_text) = match self.status {
            SyncStatus::Connecting => (rgb(0xf9e2af), "Connecting…"),
            SyncStatus::Connected => (rgb(0xa6e3a1), "Synced"),
            SyncStatus::Offline => (rgb(0xf38ba8), "Offline (unsynced)"),
        };
        let status = div()
            .flex()
            .items_center()
            .gap_2()
            .py_1()
            .child(div().w(px(10.)).h(px(10.)).rounded_full().bg(dot_color))
            .child(div().text_color(rgb(0xa6adc8)).child(status_text));

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
            .child(Self::row("Sync", status))
    }
}

fn main() {
    // GPUI must own the main thread, so build an explicit tokio runtime for
    // the sync actor instead of #[tokio::main]. It lives here on main's stack
    // for the duration of the app.
    let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    let (changes, events) = sync::start(runtime.handle());

    gpui_platform::application().run(move |cx: &mut App| {
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
                |_window, cx| cx.new(|cx| SyncitApp::new(cx, changes, events)),
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
