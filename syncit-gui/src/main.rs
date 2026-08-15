mod sync;
mod text_input;

use automerge::{AutoCommit, sync::SyncDoc};
use gpui::{
    App, AsyncApp, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyBinding, Task,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use syncit_core::SyncItDoc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::sync::{NetCmd, NetEvent, SyncStatus};
use crate::text_input::TextInput;

actions!(syncit, [Quit]);

struct SyncitApp {
    focus_handle: FocusHandle,
    name: Entity<TextInput>,
    desc: Entity<TextInput>,
    active: bool,
    count: i64,
    // The GUI owns the doc: edits apply here synchronously, and the sync
    // protocol pumps over the network channels between frames.
    doc: AutoCommit,
    sync_state: automerge::sync::State,
    status: SyncStatus,
    sync_enabled: bool,
    net_tx: UnboundedSender<NetCmd>,
    // Last text values pushed into the inputs, used to tell genuine local
    // edits apart from remote updates we applied to the inputs ourselves.
    last_name: String,
    last_desc: String,
    _net_task: Task<()>,
}

impl SyncitApp {
    fn new(
        cx: &mut Context<Self>,
        net_tx: UnboundedSender<NetCmd>,
        mut events: UnboundedReceiver<NetEvent>,
    ) -> Self {
        let name = cx.new(|cx| TextInput::new(cx, "Name…", false));
        let desc = cx.new(|cx| TextInput::new(cx, "Description…", true));

        cx.observe(&name, |this: &mut Self, input, cx| {
            let content = input.read(cx).content.to_string();
            if content != this.last_name {
                this.last_name = content.clone();
                this.edit(cx, |sd| sd.name = content);
            }
        })
        .detach();
        cx.observe(&desc, |this: &mut Self, input, cx| {
            let content = input.read(cx).content.to_string();
            if content != this.last_desc {
                this.last_desc = content.clone();
                this.edit(cx, |sd| sd.desc.update(&content));
            }
        })
        .detach();

        let net_task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            while let Some(event) = events.recv().await {
                let alive = this
                    .update(cx, |app, cx| app.on_net_event(event, cx))
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
            doc: AutoCommit::new(),
            sync_state: automerge::sync::State::new(),
            status: SyncStatus::Connecting,
            sync_enabled: true,
            net_tx,
            last_name: String::new(),
            last_desc: String::new(),
            _net_task: net_task,
        }
    }

    /// Apply a local edit to the doc, then push it out and refresh the view.
    /// Before the first sync the doc is empty and can't hydrate; seed it with
    /// the default doc so early offline edits still land (automerge merges
    /// with the server's copy once we connect).
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut SyncItDoc)) {
        let mut sd: SyncItDoc =
            autosurgeon::hydrate(&self.doc).unwrap_or_else(|_| SyncItDoc::new());
        f(&mut sd);
        if let Err(err) = autosurgeon::reconcile(&mut self.doc, &sd) {
            eprintln!("sync: failed to apply local edit: {err:#}");
        }
        self.pump();
        self.refresh_from_doc(cx);
        cx.notify();
    }

    /// Send every sync message the doc currently has queued up.
    fn pump(&mut self) {
        if self.status != SyncStatus::Connected {
            return;
        }
        while let Some(msg) = self.doc.sync().generate_sync_message(&mut self.sync_state) {
            if self.net_tx.send(NetCmd::Send(msg.encode())).is_err() {
                break;
            }
        }
    }

    fn toggle_sync(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.sync_enabled = !self.sync_enabled;
        self.net_tx
            .send(NetCmd::SetEnabled(self.sync_enabled))
            .ok();
        cx.notify();
    }

    fn on_net_event(&mut self, event: NetEvent, cx: &mut Context<Self>) {
        match event {
            NetEvent::Status(status) => {
                self.status = status;
                if status == SyncStatus::Connected {
                    // Sync protocol state is per-connection; start fresh.
                    self.sync_state = automerge::sync::State::new();
                    self.pump();
                }
            }
            NetEvent::Recv(bytes) => match automerge::sync::Message::decode(&bytes) {
                Ok(msg) => {
                    let heads_before = self.doc.get_heads();
                    if let Err(err) = self
                        .doc
                        .sync()
                        .receive_sync_message(&mut self.sync_state, msg)
                    {
                        eprintln!("sync: failed to apply sync message: {err:#}");
                    }
                    self.pump();
                    if self.doc.get_heads() != heads_before {
                        self.refresh_from_doc(cx);
                    }
                }
                Err(err) => eprintln!("sync: bad sync message: {err:#}"),
            },
        }
        cx.notify();
    }

    /// Update the view state from the doc, leaving the text inputs alone
    /// unless their content actually changed.
    fn refresh_from_doc(&mut self, cx: &mut Context<Self>) {
        let Ok(sd) = autosurgeon::hydrate::<_, SyncItDoc>(&self.doc) else {
            return; // empty doc, nothing synced yet
        };
        self.active = sd.active;
        self.count = sd.count.value();
        if sd.name != self.last_name {
            self.last_name = sd.name.clone();
            self.name
                .update(cx, |input, cx| input.set_content(sd.name, cx));
        }
        let desc = sd.desc.as_str().to_string();
        if desc != self.last_desc {
            self.last_desc = desc.clone();
            self.desc.update(cx, |input, cx| input.set_content(desc, cx));
        }
    }

    fn toggle_active(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let active = !self.active;
        self.edit(cx, |sd| sd.active = active);
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
                this.edit(cx, |sd| sd.count.increment(delta));
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
            SyncStatus::Paused => (rgb(0x6c7086), "Sync off (unsynced)"),
        };
        let toggle = div()
            .id("sync-toggle")
            .px_2()
            .py_0p5()
            .rounded_md()
            .bg(rgb(0x45475a))
            .hover(|style| style.bg(rgb(0x585b70)))
            .cursor_pointer()
            .child(if self.sync_enabled { "Pause" } else { "Resume" })
            .on_click(cx.listener(Self::toggle_sync));
        let status = div()
            .flex()
            .items_center()
            .gap_2()
            .py_1()
            .child(div().w(px(10.)).h(px(10.)).rounded_full().bg(dot_color))
            .child(div().text_color(rgb(0xa6adc8)).child(status_text))
            .child(toggle);

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
    // the network shuttle instead of #[tokio::main]. It lives here on main's
    // stack for the duration of the app.
    let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    let (net_tx, events) = sync::start(runtime.handle());

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
                |_window, cx| cx.new(|cx| SyncitApp::new(cx, net_tx, events)),
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
