//! Reuse selected nodes across the Design panel's repeated mixed-value reads.

use std::cell::RefCell;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use lizard_doc::{Document, Node, NodeId};

struct Shown {
    doc: Weak<Document>,
    selection: Vec<NodeId>,
    nodes: Vec<Option<Arc<Node>>>,
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

#[cfg(test)]
static BYPASS_FOR_TEST: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn bypass_for_test(bypass: bool) {
    BYPASS_FOR_TEST.store(bypass, Ordering::Relaxed);
}

pub(crate) fn show(doc: &Arc<Document>, selection: &[NodeId]) {
    SHOWN.with(|slot| {
        let mut slot = slot.borrow_mut();
        // The Weak keeps the control block allocated, so another Document cannot reuse this
        // pointer while the cached entry exists.
        if slot.as_ref().is_some_and(|kept| std::ptr::eq(kept.doc.as_ptr(), Arc::as_ptr(doc)) && kept.selection == selection) {
            return;
        }
        *slot = Some(Shown {
            doc: Arc::downgrade(doc),
            selection: selection.to_vec(),
            nodes: selection.iter().map(|id| doc.node_arc(*id).cloned()).collect(),
        });
    });
}

pub(crate) fn with_nodes<R>(doc: &Document, selection: &[NodeId], f: impl FnOnce(&mut dyn Iterator<Item = Option<&Node>>) -> R) -> R {
    #[cfg(test)]
    if BYPASS_FOR_TEST.load(Ordering::Relaxed) {
        return f(&mut selection.iter().map(|id| doc.node(*id)));
    }
    SHOWN.with(|slot| {
        let slot = slot.borrow();
        match slot.as_ref().filter(|kept| std::ptr::eq(kept.doc.as_ptr(), doc) && kept.selection == selection) {
            Some(kept) => f(&mut kept.nodes.iter().map(Option::as_deref)),
            None => f(&mut selection.iter().map(|id| doc.node(*id))),
        }
    })
}
