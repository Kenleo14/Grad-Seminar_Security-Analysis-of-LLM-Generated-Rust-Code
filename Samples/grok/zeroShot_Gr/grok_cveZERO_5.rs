use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, RwLock, Weak,
};
use std::fmt;

/// Trait for death notification callbacks.
pub trait DeathNotification: Send + Sync + fmt::Debug {
    /// Called when the associated Binder node dies.
    fn notify(&self, cookie: u64);
}

/// A single entry in the intrusive singly-linked list of death notifications.
/// The `next` field provides the intrusive link, protected by a `Mutex` for safe concurrent modification.
#[derive(Debug)]
struct DeathEntry {
    cookie: u64,
    recipient: Arc<dyn DeathNotification>,
    next: Mutex<Option<Arc<DeathEntry>>>,
}

/// A Binder node, holding reference count, death notification list, etc.
#[derive(Debug)]
pub struct BinderNode {
    handle: u64,
    refs: AtomicUsize,  // Logical reference count (separate from Arc strong count)
    death_head: Mutex<Option<Arc<DeathEntry>>>,
}

impl BinderNode {
    /// Creates a new Binder node with initial logical refcount of 1 (for the registry).
    pub fn new(handle: u64) -> Self {
        Self {
            handle,
            refs: AtomicUsize::new(1),
            death_head: Mutex::new(None),
        }
    }

    /// Increments the logical reference count.
    pub fn inc_ref(&self) {
        self.refs.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements the logical reference count. If it reaches zero, triggers death notifications.
    pub fn dec_ref(&self) {
        let prev = self.refs.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            self.notify_deaths();
        }
    }

    /// Registers a death notification recipient (prepends to the intrusive list).
    pub fn register_death(&self, recipient: Arc<dyn DeathNotification>, cookie: u64) {
        let mut head_guard = self.death_head.lock().unwrap();
        let new_entry = Arc::new(DeathEntry {
            cookie,
            recipient,
            next: Mutex::new(head_guard.clone()),
        });
        *head_guard = Some(new_entry);
    }

    /// Unregisters a death notification by cookie (traverses and unlinks from intrusive list).
    pub fn unregister_death(&self, cookie: u64) {
        let mut head_guard = self.death_head.lock().unwrap();
        let mut prev = None::<Arc<DeathEntry>>;
        let mut head_ref = head_guard.as_mut();
        while let Some(ref mut current_arc) = *head_ref {
            if current_arc.cookie == cookie {
                // Unlink: take next and update prev or head
                let next = current_arc.next.lock().unwrap().take();
                if let Some(prev_arc) = prev {
                    *prev_arc.next.lock().unwrap() = next;
                } else {
                    *head_ref = next;
                }
                return;
            }
            // Advance
            prev = Some(current_arc.clone());
            let next = current_arc.next.lock().unwrap().clone();
            *head_ref = next;
        }
    }

    /// Detaches the death list and notifies all recipients (safe concurrent snapshot).
    fn notify_deaths(&self) {
        let old_head = self.death_head.lock().unwrap().replace(None);
        let mut current = Some(old_head);
        let mut to_notify = Vec::new();
        while let Some(entry) = current {
            to_notify.push((entry.recipient.clone(), entry.cookie));
            current = entry.next.lock().unwrap().clone();
        }
        for (recipient, cookie) in to_notify {
            recipient.notify(cookie);
        }
    }
}

/// Central context managing Binder nodes by handle.
/// Uses RwLock<HashMap> for lookups, with deferred cleanup via temporary list for high-concurrency releases.
#[derive(Debug)]
pub struct BinderContext {
    nodes: RwLock<HashMap<u64, Arc<BinderNode>>>,
    next_handle: AtomicU64,
    pending_cleanup: Mutex<VecDeque<Arc<BinderNode>>>,
}

impl BinderContext {
    /// Creates a new Binder context.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            nodes: RwLock::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
            pending_cleanup: Mutex::new(VecDeque::new()),
        })
    }

    /// Allocates a new node and returns its handle (auto-assigned).
    pub fn create_node(&self) -> u64 {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let node = Arc::new(BinderNode::new(handle));
        self.nodes.write().unwrap().insert(handle, node);
        handle
    }

    /// Looks up a node by handle and returns a shared reference (Arc clone).
    /// Does *not* increment logical refcount; caller must call `node.inc_ref()` if holding for use.
    pub fn get_node(&self, handle: u64) -> Option<Arc<BinderNode>> {
        self.nodes.read().unwrap().get(&handle).cloned()
    }

    /// Registers a death notification for the node at the given handle.
    pub fn register_death(
        &self,
        handle: u64,
        recipient: Arc<dyn DeathNotification>,
        cookie: u64,
    ) {
        if let Some(node) = self.get_node(handle) {
            node.register_death(recipient, cookie);
        }
    }

    /// Unregisters a death notification by cookie for the node at the given handle.
    pub fn unregister_death(&self, handle: u64, cookie: u64) {
        if let Some(node) = self.get_node(handle) {
            node.unregister_death(cookie);
        }
    }

    /// Releases a node handle: removes from active registry and moves to temporary cleanup list.
    /// This handles high-concurrency releases by minimizing RwLock hold time and deferring dec_ref.
    pub fn release_node(&self, handle: u64) {
        if let Some(node) = self.nodes.write().unwrap().remove(&handle) {
            self.pending_cleanup.lock().unwrap().push_back(node);
        }
    }

    /// Processes the temporary cleanup list: performs logical dec_ref on batched nodes.
    /// Call this periodically to finalize releases and trigger death notifications where appropriate.
    /// Ensures memory stability by batching and avoiding immediate drops under lock.
    pub fn cleanup(&self) {
        let mut pend_guard = self.pending_cleanup.lock().unwrap();
        let mut batch: Vec<Arc<BinderNode>> = Vec::with_capacity(pend_guard.len());
        pend_guard.append(&mut batch);
        std::mem::swap(&mut *pend_guard, &mut batch);
        drop(pend_guard);
        for node in batch {
            node.dec_ref();
        }
        // Remaining Arcs in batch drop here; node drops if no other holders.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    struct TestDeathRecipient {
        tx: std::sync::mpsc::Sender<u64>,
    }

    impl DeathNotification for TestDeathRecipient {
        fn notify(&self, cookie: u64) {
            let _ = self.tx.send(cookie);
        }
    }

    impl fmt::Debug for TestDeathRecipient {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TestDeathRecipient").finish()
        }
    }

    #[test]
    fn test_basic_node_lifecycle() {
        let ctx = BinderContext::new();
        let handle = ctx.create_node();
        let node = ctx.get_node(handle).unwrap();
        assert_eq!(node.handle, handle);

        let (tx, rx) = channel();
        let recip = Arc::new(TestDeathRecipient { tx });
        ctx.register_death(handle, recip.clone(), 42);

        ctx.release_node(handle);
        ctx.cleanup();
        assert!(rx.try_recv().is_ok());
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn test_register_unregister() {
        let ctx = BinderContext::new();
        let handle = ctx.create_node();
        let node = ctx.get_node(handle).unwrap();

        let (tx1, rx1) = channel();
        let recip1 = Arc::new(TestDeathRecipient { tx: tx1 });
        ctx.register_death(handle, recip1, 1);

        ctx.unregister_death(handle, 1);

        ctx.release_node(handle);
        ctx.cleanup();
        assert!(rx1.try_recv().is_err());  // Should not notify
    }

    #[test]
    fn test_logical_refs() {
        let ctx = BinderContext::new();
        let handle = ctx.create_node();
        let node = ctx.get_node(handle).unwrap();
        node.inc_ref();  // Simulate client hold
        node.inc_ref();  // Another

        ctx.release_node(handle);  // Registry dec, refs=2
        ctx.cleanup();  // Does dec_ref, refs=1 now

        node.dec_ref();  // refs=0, notify
        drop(node);
    }

    #[test]
    fn test_concurrent_releases() {
        let ctx = Arc::new(BinderContext::new());
        let handles: Vec<_> = (0..100).map(|_| ctx.create_node()).collect();

        // Simulate high-concurrency releases
        let mut threads = vec![];
        for handle in handles {
            let ctx_clone = ctx.clone();
            threads.push(std::thread::spawn(move || {
                ctx_clone.release_node(handle);
            }));
        }
        for t in threads {
            t.join().unwrap();
        }

        ctx.cleanup();  // Batch cleanup
    }
}
