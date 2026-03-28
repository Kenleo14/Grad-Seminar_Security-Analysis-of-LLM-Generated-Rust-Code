//! Simplified Android Binder Node management system.
//! Implements intrusive linked-list death notifications with high-concurrency
//! release support via temporary-list cleanup. Memory stability is guaranteed
//! by exclusive ownership of the temp list after the O(1) head swap.

use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// A death notification entry. The intrusive links are embedded directly in
/// this struct (the "intrusive" part). Callers allocate via `Box::new` and
/// pass ownership to `BinderNode::register_death`.
pub struct DeathNotification {
    id: u64,
    callback: Option<Box<dyn FnOnce(u64) + Send>>,
    links: ListLinks,
}

/// Private intrusive links. Not exposed to callers.
#[derive(Default)]
struct ListLinks {
    next: Option<NonNull<DeathNotification>>,
    prev: Option<NonNull<DeathNotification>>,
}

impl DeathNotification {
    /// Create a new death notification with an ID and a one-shot callback.
    /// The callback will be invoked exactly once when the owning node is released.
    pub fn new(id: u64, callback: impl FnOnce(u64) + Send + 'static) -> Self {
        Self {
            id,
            callback: Some(Box::new(callback)),
            links: ListLinks::default(),
        }
    }
}

/// Internal state of a Binder node, protected by a mutex.
struct BinderNodeInner {
    /// Head of the intrusive doubly-linked list of death notifications.
    /// `None` when empty. First node's `prev` is always `None`.
    death_list_head: Option<NonNull<DeathNotification>>,
}

/// A Binder node. Share via `Arc<BinderNode>` to simulate reference-counted
/// Binder objects. When the last `Arc` is dropped, death notifications are
/// processed.
pub struct BinderNode {
    inner: Mutex<BinderNodeInner>,
}

impl BinderNode {
    /// Create a new Binder node.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BinderNodeInner {
                death_list_head: None,
            }),
        }
    }

    /// Register a death notification. Ownership of the `Box` is transferred
    /// to the list (via `Box::into_raw`). The callback will fire when this
    /// node is released.
    pub fn register_death(&self, death: Box<DeathNotification>) {
        let entry = NonNull::new(Box::into_raw(death)).expect("Box::into_raw never returns null");
        let mut guard = self.inner.lock().unwrap();
        add_to_list(&mut guard.death_list_head, entry);
    }
}

/// Helper: add an entry to the front of the intrusive list (O(1)).
/// # Safety
/// - `entry` must be a valid, non-null pointer obtained from `Box::into_raw`.
/// - The caller must ensure the pointed-to memory lives until it is removed
///   from the list and converted back via `Box::from_raw`.
fn add_to_list(head: &mut Option<NonNull<DeathNotification>>, entry: NonNull<DeathNotification>) {
    unsafe {
        let entry_mut = entry.as_mut();
        entry_mut.links.next = *head;
        entry_mut.links.prev = None;

        if let Some(old_head) = *head {
            old_head.as_mut().links.prev = Some(entry);
        }

        *head = Some(entry);
    }
}

/// Process an entire death list (the temporary list after release).
/// Runs *outside* any lock for maximum concurrency.
/// Consumes the list by turning raw pointers back into `Box` and dropping them.
fn process_death_notifications(mut head: Option<NonNull<DeathNotification>>) {
    while let Some(node_ptr) = head {
        unsafe {
            // Read the next pointer *before* any potential drop or callback.
            let next = node_ptr.as_ref().links.next;

            // Invoke the callback (if any). This runs without holding the node lock.
            let node = &mut *node_ptr.as_ptr();
            if let Some(cb) = node.callback.take() {
                cb(node.id);
            }

            // Reclaim ownership and drop the notification.
            // Memory is stable here because this thread exclusively owns the temp list.
            let _ = Box::from_raw(node_ptr.as_ptr());

            head = next;
        }
    }
}

impl Drop for BinderNode {
    fn drop(&mut self) {
        let mut guard = self.inner.lock().unwrap();

        // === Critical section (kept tiny for high concurrency) ===
        // Move the entire death list to a temporary list in O(1) time.
        // No traversal, no per-node work while the lock is held.
        let mut temp_head = None;
        std::mem::swap(&mut guard.death_list_head, &mut temp_head);
        // =============================================================

        // Explicitly release the lock *before* processing callbacks.
        // This is the core of the "high-concurrency release" pattern.
        drop(guard);

        // Process the temporary list outside the lock.
        // Callbacks may perform arbitrary work without blocking other threads.
        process_death_notifications(temp_head);
    }
}