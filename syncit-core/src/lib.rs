use autosurgeon::{Counter, Hydrate, Reconcile, Text};

#[derive(Debug, Clone, Hydrate, Reconcile)]
pub struct SyncItDoc {
    pub name: String,
    pub active: bool,
    pub count: Counter,
    pub desc: Text,
}
