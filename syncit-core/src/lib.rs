use autosurgeon::{Counter, Hydrate, Reconcile, Text};

#[derive(Debug, Clone, Hydrate, Reconcile)]
pub struct SyncItDoc {
    pub name: String,
    pub active: bool,
    pub count: Counter,
    pub desc: Text,
}

impl SyncItDoc {
    pub fn new() -> Self {
        Self {
            name: "demo".into(),
            active: true,
            count: Counter::with_value(2),
            desc: "Todo...".into(),
        }
    }
}
